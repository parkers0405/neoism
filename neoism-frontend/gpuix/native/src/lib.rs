//! Native host bridge: shared Rust UI/editor engines produce GPUIX paint data.
//! Host connection helpers manage local SSH/system configuration; document
//! services and interactive PTY ownership remain in the workspace daemon.
use neoism_ui::editor::code::layout::{self, VisualRow, TAB_DISPLAY_WIDTH};
use neoism_ui::editor::code::{
    CodeInputMode, CodeMode, CodeMotion, CodePane, CodePaneGeometry,
};
use neoism_ui::editor::markdown::vim::{VimAction, VimKeyFeed};
use neoism_ui::panels::file_tree::{FileTree, GitStatus, NodeKind, TreeEntry};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{c_char, CString},
    path::PathBuf,
    time::Instant,
};
mod pickers;
mod ssh_transport;
use ssh_transport::original as ssh_hosts;
#[path = "layout.rs"]
mod host_layout;
mod markdown;
mod agent_timeline;
mod paint_canvas;
mod ghostty;
mod editor_lsp;
mod editor_raster;
mod editor_collaboration;
mod editor_presence;
mod presence_cache;
mod finder;
mod command_catalog;
mod chrome;
mod notifications;
mod diagnostic_popup;
mod presence;
mod keyboard_policy;
mod tree_glyphs;
mod menu_actions;
mod server_management;
mod service_process;
mod fast_servers;
mod agent_composer;
mod splash;
mod wordmark;
mod breadcrumbs;
mod git_panel;
mod git_review;
mod git_confirm;

struct Editor {
    collaboration: Option<editor_collaboration::EditorCollaboration>,
    peers: Vec<neoism_protocol::crdt::CrdtPeerPresence>,
    peer_paint: editor_presence::PeerPainter,
    pane: CodePane,
    register: String,
    clipboard: Option<String>,
    host_action: Option<&'static str>,
    width: f32,
    height: f32,
    cell: f32,
    row: f32,
    trail: neoism_ui::panels::trail_cursor::TrailCursor,
    last_frame: Instant,
    last_cursor: (usize, usize),
    lsp: editor_lsp::EditorLsp,
    compact: bool,
    raster: editor_raster::EditorRaster,
    presence: presence_cache::PresenceCache,
}
#[derive(Default)]
struct Engines {
    next: u32,
    trees: HashMap<u32, FileTree>,
    editors: HashMap<u32, Editor>,
}
thread_local! { static ENGINES: RefCell<Engines> = RefCell::new(Engines::default()); }
fn n(v: &Value, key: &str, default: f64) -> f32 {
    v[key].as_f64().unwrap_or(default) as f32
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

impl Editor {
    fn clone_view(&self) -> Self {
        let mut pane = CodePane::new(self.pane.path.clone(), "");
        pane.buffer = self.pane.buffer.clone();
        pane.highlight = self.pane.highlight.clone();
        pane.input_mode = self.pane.input_mode;
        pane.remote_source = true;
        pane.wrap = self.pane.wrap;
        pane.search_highlight = self.pane.search_highlight.clone();
        pane.restore_host_scroll(self.pane.scroll_x, self.pane.scroll_y);
        Self { collaboration:None, peers:self.peers.clone(), peer_paint:Default::default(), pane, register: self.register.clone(), clipboard: None, host_action: None,
            width: self.width, height: self.height, cell: self.cell, row: self.row,
            trail: neoism_ui::panels::trail_cursor::TrailCursor::new(),
            last_frame: Instant::now(), last_cursor: self.last_cursor, lsp: Default::default(), compact: self.compact, raster: self.raster.clone_settings(), presence:Default::default() }
    }
    fn snapshot(&mut self) -> Value {
        let collaboration=self.collaboration.as_mut().map(|binding|binding.snapshot(&mut self.pane.buffer));
        json!({"collaboration":collaboration,"version":1,"text":self.pane.buffer.text_for_disk(),"buffer":self.pane.buffer.recovery_snapshot(),
            "inputMode":self.pane.input_mode,"wrap":self.pane.wrap,"scrollX":self.pane.scroll_x,"scrollY":self.pane.scroll_y,
            "register":self.register,"searchHighlight":self.pane.search_highlight})
    }
    fn restore(&mut self, snapshot: &Value) -> Result<(), String> {
        if snapshot["version"].as_u64() != Some(1) { return Err("Unsupported editor snapshot version".into()); }
        let buffer = neoism_ui::editor::code::CodeBuffer::from_recovery_snapshot(snapshot["buffer"].clone())?;
        let mut pane = CodePane::new(self.pane.path.clone(), &buffer.text_for_disk());
        pane.buffer = buffer;
        pane.input_mode = serde_json::from_value(snapshot["inputMode"].clone()).map_err(|_|"Invalid editor input mode")?;
        pane.remote_source = true;
        pane.wrap = snapshot["wrap"].as_bool().unwrap_or(true);
        pane.search_highlight = snapshot["searchHighlight"].as_str().map(str::to_owned);
        pane.restore_host_scroll(n(snapshot,"scrollX",0.0), n(snapshot,"scrollY",0.0));
        self.collaboration=if snapshot["collaboration"].is_null(){None}else{Some(editor_collaboration::EditorCollaboration::restore(&snapshot["collaboration"],&mut pane.buffer)?)};
        self.pane = pane;
        self.presence.clear();
        self.register = text(snapshot,"register").to_owned();
        self.clipboard = None;
        self.host_action = None;
        self.trail = neoism_ui::panels::trail_cursor::TrailCursor::new();
        Ok(())
    }
    /// Advance only the already-positioned cursor. Document input, scrolling,
    /// collaboration and LSP requests still go through a full frame; the host
    /// uses this path solely for the tail of a settled viewport's cursor spring.
    fn cursor_frame(&mut self) -> Value {
        let row = self.pane.geometry.row_h.max(1.0);
        let cell = self.cell.max(1.0);
        let caret_h = (self.row / 1.5 * 1.2).min(row).round();
        let now = Instant::now();
        self.trail.animate(cell, caret_h, now.duration_since(self.last_frame).as_secs_f32());
        self.last_frame = now;
        json!({"cursorOnly":true,"cursorQuad":self.trail.quad_points(),
            "cursorHold":self.trail.blink_hold_visible(neoism_ui::editor::optimistic::DEFAULT_BLINK_PERIOD_MS),
            "animating":self.trail.is_animating()})
    }
    fn frame(&mut self) -> Value {
        if let Some(collaboration)=self.collaboration.as_mut(){collaboration.drain(&mut self.pane.buffer);}
        self.lsp.session.pump(&mut self.pane);
        let pane = &mut self.pane;
        let row = self.row.max(1.0);
        let cell = self.cell.max(1.0);
        let fits = (self.height / row).floor().max(1.0);
        let row = if fits >= 8.0 { self.height / fits } else { row };
        let gutter = 20.0 + layout::gutter_digits(pane.buffer.line_count()) as f32 * cell;
        let text_x = gutter + 8.0;
        let view = neoism_ui::editor::code::viewport::update_viewport(
            pane,
            0.0,
            self.width,
            text_x,
            cell,
            row,
            fits * row,
        );
        let first = (pane.scroll_y / row).floor().max(0.0) as usize;
        pane.geometry = CodePaneGeometry {
            rect: [0.0, 0.0, self.width, self.height],
            text_x,
            gutter_w: gutter,
            cell_w: cell,
            row_h: row,
            first_row: first,
            scroll_y: pane.scroll_y,
            scroll_x: pane.scroll_x,
            wrap: view.wrap.clone(),
            ..Default::default()
        };
        pane.highlight.refresh(&pane.buffer, pane.language);
        let selection = pane.buffer.selection_range();
        let mut lines = Vec::new();
        let mut search_hits = Vec::new();
        self.raster.begin_frame();
        for visual in first..(first + fits.ceil() as usize + 2).min(view.total_rows) {
            let Some(VisualRow::Source { line, segment }) = view.wrap.row_kind(visual)
            else {
                continue;
            };
            let Some(seg) = view.wrap.segment(line, segment) else {
                continue;
            };
            let source = &pane.buffer.lines[line];
            if let Some(pattern) =
                pane.search_highlight.as_ref().filter(|p| !p.is_empty())
            {
                for (start, found) in source.match_indices(pattern.as_str()) {
                    let lo = start.max(seg.byte_start);
                    let hi = (start + found.len()).min(seg.byte_end);
                    if lo >= hi {
                        continue;
                    }
                    let x = seg.visual_indent
                        + layout::display_col_for_byte(source, lo, TAB_DISPLAY_WIDTH)
                            .saturating_sub(seg.source_col);
                    let width =
                        layout::display_col_for_byte(source, hi, TAB_DISPLAY_WIDTH)
                            - layout::display_col_for_byte(source, lo, TAB_DISPLAY_WIDTH);
                    search_hits.push(json!({"x":text_x+x as f32*cell-pane.scroll_x,"y":visual as f32*row-pane.scroll_y,"width":width as f32*cell}));
                }
            }
            let selected = selection.and_then(|(a, b)| {
                if line < a.line || line > b.line {
                    None
                } else {
                    Some((
                        if line == a.line { a.col } else { 0 },
                        if line == b.line { b.col } else { source.len() },
                    ))
                }
            });
            for extra in pane.buffer.extra_carets.iter().filter(|c| c.line == line) {
                if let Some(anchor) = extra.anchor.filter(|a| a.line == line) {
                    let lo = anchor.col.min(extra.col).max(seg.byte_start);
                    let hi = anchor.col.max(extra.col).min(seg.byte_end);
                    if lo < hi {
                        let start = seg.visual_indent
                            + layout::display_col_for_byte(source, lo, TAB_DISPLAY_WIDTH)
                                .saturating_sub(seg.source_col);
                        let width =
                            layout::display_col_for_byte(source, hi, TAB_DISPLAY_WIDTH)
                                - layout::display_col_for_byte(
                                    source,
                                    lo,
                                    TAB_DISPLAY_WIDTH,
                                );
                        search_hits.push(json!({"x":text_x+start as f32*cell-pane.scroll_x,"y":visual as f32*row-pane.scroll_y,"width":width as f32*cell,"selection":true}));
                    }
                }
            }
            let spans = neoism_ui::editor::code::styled_runs_with_syntax(
                source,
                pane.highlight.line_runs(line),
                pane.language,
                selected,
                pane.diagnostics.get(&line).map_or(&[][..], Vec::as_slice),
                &[],
            );
            let mut glyph_runs = Vec::new();
            let runs: Vec<Value> = spans.into_iter().filter_map(|span| {
                let lo = span.start.max(seg.byte_start); let hi = span.end.min(seg.byte_end);
                if lo >= hi { return None }
                let col = seg.visual_indent + layout::display_col_for_byte(source, lo, TAB_DISPLAY_WIDTH).saturating_sub(seg.source_col);
                let text=layout::expand_tabs_from(&source[lo..hi],layout::display_col_for_byte(source,lo,TAB_DISPLAY_WIDTH),TAB_DISPLAY_WIDTH);
                let width=(layout::display_col_for_byte(source,hi,TAB_DISPLAY_WIDTH)-layout::display_col_for_byte(source,lo,TAB_DISPLAY_WIDTH)) as f32*cell;
                let glyph_x=text_x+col as f32*cell;
                let x=glyph_x-pane.scroll_x;
                let token=format!("{:?}",span.token);
                let severity=span.severity.map(|s|format!("{:?}",s));
                if self.raster.enabled() { glyph_runs.push(editor_raster::RowRun{text:text.clone(),x:glyph_x,width,token:token.clone()}); }
                Some(if self.compact {json!([text,x,width,token,span.selected,severity])}
                    else {json!({"text":text,"width":width,"x":x,"token":token,"selected":span.selected,"severity":severity})})
            }).collect();
            let mut painted=json!({"line":line,"continuation":segment>0,"y":visual as f32 * row-pane.scroll_y,"runs":runs});
            if self.raster.enabled() { if let Some(raster)=self.raster.row(glyph_runs,row,pane.scroll_x){painted["raster"]=raster;} }
            lines.push(painted);
        }
        let caret_h = (self.row / 1.5 * 1.2).min(row).round();
        let extra_carets:Vec<Value> = pane.buffer.extra_carets.iter().filter_map(|extra| {
            let source=pane.buffer.lines.get(extra.line)?;
            let (segment,col)=view.wrap.visual_position(extra.line,source,extra.col.min(source.len()),TAB_DISPLAY_WIDTH);
            let visual=view.wrap.first_row_of_line(extra.line)+segment;
            let y=visual as f32*row-pane.scroll_y;
            Some(json!({"x":text_x+col as f32*cell-pane.scroll_x,"y":y+((row-caret_h)*0.5).max(0.0).round(),"height":caret_h,"width":if pane.buffer.mode==CodeMode::Insert{2.0}else{cell}}))
        }).collect();
        let cursor_y = view.cursor_vrow as f32 * row - pane.scroll_y;
        let cursor_x = text_x + view.cursor_local_col as f32 * cell - pane.scroll_x;
        let caret_x = cursor_x.clamp(0.0, (self.width - cell).max(0.0));
        let caret_y = (cursor_y + ((row - caret_h) * 0.5).max(0.0).round())
            .clamp(0.0, (self.height - caret_h).max(0.0));
        let was_animating = self.trail.is_animating();
        self.trail.set_cursor_shape(pane.cursor_shape());
        // Match the original Code overlay: its destination follows the
        // rendered caret every display frame (the terminal has a separate
        // scroll-only/no-jump policy).
        self.trail.set_destination(caret_x, caret_y, cell, caret_h);
        self.last_cursor = (pane.buffer.cursor_line, pane.buffer.cursor_col);
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        // An on-demand host has no rendered frames during idle. That idle
        // time predates this transition and must not consume its spring.
        self.trail.animate(cell, caret_h, if was_animating { dt } else { 0.0 });
        let lsp = self.lsp.frame(pane);
        let presence=self.presence.frame(pane.buffer.revision,&pane.buffer.lines,(pane.buffer.cursor_line,pane.buffer.cursor_col),selection.map(|(a,b)|((a.line,a.col),(b.line,b.col))),pane.buffer.mode==CodeMode::Insert);
        json!({"peerCommands":self.peer_paint.frame(pane,&self.peers),"remotePeers":self.peers,"collaborationBound":pane.buffer.doc_history_bound(),"presence":presence,"packedRuns":self.compact,"lsp":lsp,"lines":lines,"searchHits":search_hits,"extraCarets":extra_carets,"rowHeight":row,"cellWidth":cell,"textX":text_x,"gutterWidth":gutter,
            "cursor":{"x":cursor_x,"y":cursor_y,"line":pane.buffer.cursor_line,"col":pane.buffer.cursor_col,
              "shape":format!("{:?}",pane.cursor_shape()),"visible":cursor_y+row>0.0 && cursor_y<self.height},
            "mode":format!("{:?}",pane.buffer.mode),"wrap":pane.wrap,"vim":pane.input_mode==CodeInputMode::Vim,"dirty":pane.buffer.is_dirty(),"revision":pane.buffer.revision,
            "totalLines":pane.buffer.line_count(),"pendingKeys":pane.buffer.vim.pending.showcmd(),
            "scrollY":pane.scroll_y,"contentHeight":view.content_h,"scrollAnimating":view.scroll_animating,"animating":view.scroll_animating||self.trail.is_animating(),
            "clipboard":self.clipboard.take(),"hostAction":self.host_action.take(),"pendingEmpty":pane.buffer.vim.pending.is_empty()&&!pane.leader_pending,
            "cursorQuad":self.trail.quad_points(),"cursorHold":self.trail.blink_hold_visible(neoism_ui::editor::optimistic::DEFAULT_BLINK_PERIOD_MS),
            "selection":selection.map(|(a,b)|json!({"start":[a.line,a.col],"end":[b.line,b.col]}))})
    }
    fn apply_action(&mut self, action: &VimAction) {
        let applied = self
            .pane
            .buffer
            .apply_vim_action(action, Some(&self.register));
        if let Some(register) = applied.register {
            if applied.sync_clipboard {
                self.clipboard = Some(register.clone());
            }
            self.register = register;
        }
        if let Some(keys) = applied.replay_keys {
            self.pane.buffer.vim.replaying_macro = true;
            for ch in keys.chars() {
                if self.pane.buffer.mode == CodeMode::Insert {
                    break;
                }
                let visual = self.pane.buffer.mode == CodeMode::Visual;
                if let VimKeyFeed::Action(action) = self.pane.buffer.vim.feed(ch, visual)
                {
                    let applied = self
                        .pane
                        .buffer
                        .apply_vim_action(&action, Some(&self.register));
                    if let Some(register) = applied.register {
                        if applied.sync_clipboard {
                            self.clipboard = Some(register.clone());
                        }
                        self.register = register;
                    }
                }
            }
            self.pane.buffer.vim.replaying_macro = false;
        }
    }
    fn key(&mut self, v: &Value) {
        let key = text(v, "key");
        let ch = match text(v, "char") {
            "" if key == "space" => " ",
            ch => ch,
        };
        let ctrl = v["ctrl"].as_bool().unwrap_or(false);
        let shift = v["shift"].as_bool().unwrap_or(false);
        if let Some(clipboard) = v["clipboard"].as_str() {
            self.register = clipboard.to_owned();
        }
        if key == "escape" { self.pane.leader_pending = false; }
        if self.lsp.key(&mut self.pane, key, ch, ctrl, shift) { return; }
        // Desktop owns this chord outside the Vim engine. Its Code leader is
        // a bool (no timeout), and named motions / Ctrl commands precede it.
        if self.pane.input_mode == CodeInputMode::Vim && self.pane.buffer.mode != CodeMode::Insert {
            let plain = !ctrl && !v["alt"].as_bool().unwrap_or(false) && !v["cmd"].as_bool().unwrap_or(false);
            if key == "escape" { self.pane.leader_pending = false; }
            if plain && (key == "space" || ch == " ") {
                if self.pane.buffer.mode == CodeMode::Normal && self.pane.buffer.vim.pending.is_empty() {
                    self.pane.leader_pending = true;
                    return;
                }
            } else if self.pane.leader_pending && !ctrl
                && !matches!(key,"escape"|"left"|"right"|"up"|"down"|"backspace"|"pageup"|"pagedown") {
                self.pane.leader_pending = false;
                if plain {
                    let mut chars = if ch.is_empty() { key } else { ch }.chars();
                    if let (Some(character),None) = (chars.next(),chars.next()) {
                        match character.to_ascii_lowercase() {
                            'x' => self.host_action = Some("close"),
                            'a' => self.lsp.session.request_code_actions(&mut self.pane),
                            'r' => self.lsp.session.open_rename_prompt(&mut self.pane),
                            _ => {},
                        }
                    }
                }
                return;
            }
        }
        let pane = &mut self.pane;
        if ctrl && v["alt"].as_bool().unwrap_or(false) && matches!(key, "up" | "down") {
            pane.buffer.add_caret_vertical(key == "down");
            return;
        }
        if key == "escape" {
            pane.buffer.break_undo_group();
            pane.buffer.clear_extra_carets();
            pane.search_highlight = None;
            pane.buffer.vim.clear_pending();
            pane.buffer.clear_selection();
            pane.buffer.mode = if pane.input_mode == CodeInputMode::Vim {
                CodeMode::Normal
            } else {
                CodeMode::Insert
            };
            if pane.buffer.mode == CodeMode::Normal {
                pane.buffer.snap_normal_cursor();
            }
            return;
        }
        if ctrl
            && key == "d"
            && pane.input_mode == CodeInputMode::Vim
            && pane.buffer.mode != CodeMode::Insert
        {
            pane.half_page_scroll(true, pane.buffer.mode == CodeMode::Visual);
            return;
        }
        if ctrl
            && key == "u"
            && pane.input_mode == CodeInputMode::Vim
            && pane.buffer.mode != CodeMode::Insert
        {
            pane.half_page_scroll(false, pane.buffer.mode == CodeMode::Visual);
            return;
        }
        if pane.input_mode == CodeInputMode::Vim && pane.buffer.mode != CodeMode::Insert {
            let visual = pane.buffer.mode == CodeMode::Visual;
            if ch == "b"
                && pane.buffer.vim.pending.operator.is_none()
                && pane.buffer.vim.pending.stage
                    == neoism_ui::editor::markdown::vim::VimStage::Gee
            {
                pane.buffer.vim.clear_pending();
                if pane.buffer.add_caret_next_occurrence() {
                    pane.buffer.mode = CodeMode::Visual;
                    pane.buffer.follow_cursor = true;
                }
                return;
            }
            if !ctrl
                && visual
                && pane.buffer.has_extra_carets()
                && matches!(ch, "c" | "d")
            {
                pane.buffer.multi_change_selections();
                if ch == "c" {
                    pane.buffer.mode = CodeMode::Insert;
                } else {
                    pane.buffer.mode = CodeMode::Normal;
                    pane.buffer.clear_extra_carets();
                    pane.buffer.snap_normal_cursor();
                }
                return;
            }
            // Bare j/k use visual rows; counted/operator motions retain source-line semantics.
            let mapped = match key {
                "left" | "backspace" => "h",
                "right" => "l",
                "up" => "k",
                "down" => "j",
                _ => ch,
            };
            if !ctrl && pane.buffer.vim.pending.is_empty() && matches!(mapped, "j" | "k")
            {
                if pane.move_cursor_vertical_visual(mapped == "j", visual) {
                    if !visual {
                        pane.buffer.snap_normal_cursor();
                    }
                    return;
                }
            }
            let character = if ctrl {
                key.chars().next()
            } else {
                mapped.chars().next().or_else(|| {
                    if key.chars().count() == 1 {
                        key.chars().next()
                    } else {
                        None
                    }
                })
            };
            if let Some(character) = character {
                let feed = if ctrl {
                    pane.buffer.vim.feed_ctrl(character, visual)
                } else {
                    pane.buffer.vim.feed(character, visual)
                };
                match feed {
                    VimKeyFeed::Action(action) => {
                        self.apply_action(&action);
                        return;
                    }
                    VimKeyFeed::Pending | VimKeyFeed::Cancelled => return,
                    VimKeyFeed::Unhandled => {}
                }
            }
        }
        if matches!(key, "up" | "down" | "pageup" | "pagedown") {
            let down = matches!(key, "down" | "pagedown");
            let count = if matches!(key, "pageup" | "pagedown") {
                pane.geometry.viewport_rows()
            } else {
                1
            };
            if pane.move_cursor_vertical_visual_n(
                down,
                count,
                shift || pane.buffer.mode == CodeMode::Visual,
            ) {
                return;
            }
        }
        let motion = match key {
            "left" => Some(if ctrl {
                CodeMotion::WordLeft
            } else {
                CodeMotion::Left
            }),
            "right" => Some(if ctrl {
                CodeMotion::WordRight
            } else {
                CodeMotion::Right
            }),
            "up" => Some(CodeMotion::Up),
            "down" => Some(CodeMotion::Down),
            "home" => Some(if ctrl {
                CodeMotion::DocStart
            } else {
                CodeMotion::LineStartSmart
            }),
            "end" => Some(if ctrl {
                CodeMotion::DocEnd
            } else {
                CodeMotion::LineEnd
            }),
            "pageup" => Some(CodeMotion::PageUp {
                rows: pane.geometry.viewport_rows(),
            }),
            "pagedown" => Some(CodeMotion::PageDown {
                rows: pane.geometry.viewport_rows(),
            }),
            _ => None,
        };
        if let Some(motion) = motion {
            pane.buffer.apply_motion(motion, shift);
            return;
        }
        if ctrl {
            match key {
                "z" => {
                    if shift {
                        pane.buffer.redo();
                    } else {
                        pane.buffer.undo();
                    }
                }
                "y" => {
                    pane.buffer.redo();
                }
                "a" => pane.buffer.select_all(),
                "d" => {
                    pane.buffer.add_caret_next_occurrence();
                }
                _ => {}
            }
            return;
        }
        if pane.buffer.mode != CodeMode::Insert {
            return;
        }
        match key {
            "enter" | "return" => {
                if pane.buffer.has_extra_carets() {
                    pane.buffer.multi_insert_newline()
                } else {
                    pane.buffer.insert_newline()
                }
            }
            "backspace" => {
                if pane.buffer.has_extra_carets() {
                    pane.buffer.multi_backspace()
                } else {
                    pane.buffer.backspace()
                }
            }
            "delete" => pane.buffer.delete_forward(),
            "tab" => {
                if shift {
                    pane.buffer.outdent()
                } else {
                    pane.buffer.insert_tab()
                }
            }
            _ => {
                if !ch.is_empty() {
                    let mut chars = ch.chars();
                    if let (Some(c), None) = (chars.next(), chars.next()) {
                        if pane.buffer.has_extra_carets() {
                            pane.buffer.multi_insert_char(c);
                        } else {
                            pane.buffer.insert_char(c);
                        }
                    } else if pane.buffer.has_extra_carets() {
                        pane.buffer.multi_insert_text(ch);
                    } else {
                        pane.buffer.insert_text(ch);
                    }
                }
            }
        }
    }
}

fn dispatch(v: Value, engines: &mut Engines) -> Result<Value, String> {
    if v["op"]=="paint.prewarm" {
        let family=v["fontFamily"].as_str().unwrap_or("Cascadia Code NF");
        let document=v["markdownFontFamily"].as_str().unwrap_or("");
        let weight=v["fontWeight"].as_u64().unwrap_or(400) as u16;
        let extras=if document.is_empty(){vec![]}else{vec![document.to_owned()]};
        let base_ready=neoism_ui::primitives::paint_record::prewarm_fonts(family.to_owned(),weight,vec![]).map_err(|error|error.to_string())?;
        let mut ready=neoism_ui::primitives::paint_record::prewarm_fonts(family.to_owned(),weight,extras).map_err(|error|error.to_string())? && base_ready;
        // These are the exact libraries used by the always-mounted topbar,
        // status line, Git controller and notifications. Resolve them while
        // the display device initializes instead of serial font discovery
        // during the first App mount. Hidden panes still construct lazily.
        if let Some(icons)=v["iconFontFamily"].as_str() {
            for extras in [vec![icons.to_owned()],vec![icons.to_owned(),"Symbols Nerd Font Mono".into()],vec!["Symbols Nerd Font Mono".into()]] {
                ready=neoism_ui::primitives::paint_record::prewarm_fonts(family.to_owned(),weight,extras).map_err(|error|error.to_string())? && ready;
            }
            ready=neoism_ui::primitives::paint_record::prewarm_fonts("Cascadia Code NF".into(),400,vec![]).map_err(|error|error.to_string())? && ready;
        }
        return Ok(json!({"ready":ready}));
    }
    if let Some(result) = ssh_transport::dispatch(&v) { return result; }
    if let Some(result) = presence::dispatch(&v) { return result; }
    if let Some(result) = diagnostic_popup::dispatch(&v,engines) { return result; }
    if let Some(result) = notifications::dispatch(&v) { return result; }
    if let Some(result) = chrome::dispatch(&v) { return result; }
    if let Some(result) = keyboard_policy::dispatch(&v) { return result; }
    if let Some(result) = tree_glyphs::dispatch(&v) { return result; }
    if let Some(result) = breadcrumbs::dispatch(&v, engines) { return result; }
    if let Some(result) = splash::dispatch(&v) { return result; }
    if let Some(result) = agent_composer::dispatch(&v) { return result; }
    if let Some(result) = wordmark::dispatch(&v) { return result; }
    if let Some(result) = git_panel::dispatch(&v) { return result; }
    if let Some(result) = git_review::dispatch(&v, engines) { return result; }
    if let Some(result) = git_confirm::dispatch(&v) { return result; }
    if let Some(result) = server_management::dispatch(&v) { return result; }
    if let Some(result) = menu_actions::dispatch(&v) { return result; }
    if let Some(result) = command_catalog::dispatch(&v) { return result; }
    if let Some(result) = finder::dispatch(&v) { return result; }
    if let Some(result) = ghostty::dispatch(&v) { return result; }
    if let Some(result) = markdown::dispatch(&v) { return result; }
    if let Some(result) = agent_timeline::dispatch(&v) { return result; }
    if let Some(result) = agent_timeline::dispatch_panel(&v) { return result; }
    if let Some(result) = agent_timeline::dispatch_browser(&v) { return result; }
    if let Some(result) = agent_timeline::dispatch_navigator(&v) { return result; }
    if let Some(result) = host_layout::dispatch(&v) {
        return result;
    }
    if let Some(result) = pickers::dispatch(&v) {
        return result;
    }
    let op = text(&v, "op");
    let id = v["id"].as_u64().unwrap_or(0) as u32;
    match op {
        "tree.visible" => {
            let root=PathBuf::from(text(&v,"root").replace('\\',"/"));
            let hidden=v["showHidden"].as_bool().unwrap_or(true);
            let mut visible=Vec::new();
            for (relative,listing) in v["directories"].as_object().into_iter().flatten(){
                let parent=root.join(relative);
                for entry in listing["entries"].as_array().into_iter().flatten(){
                    let name=text(entry,"name");
                    if neoism_ui::panels::file_tree::entry_is_visible(&parent,name,hidden){visible.push(if relative.is_empty(){name.to_owned()}else{format!("{relative}/{name}")});}
                }
            }
            Ok(json!(visible))
        }
        "tree.new" => {
            engines.next += 1;
            let id = engines.next;
            let mut tree = FileTree::new(PathBuf::from(text(&v, "root")));
            tree.set_scale(n(&v, "scale", 1.0));
            tree.set_visible(true);
            engines.trees.insert(id, tree);
            Ok(json!({"id":id}))
        }
        op if op.starts_with("tree.") => {
            let tree = engines.trees.get_mut(&id).ok_or("Unknown tree")?;
            // `set_scale` resets the scroll/cursor springs; per-frame calls
            // carry the same scale, so apply only real changes.
            if let Some(scale)=v["scale"].as_f64(){let scale=(scale as f32).clamp(0.5,3.0);if (tree.row_height()-neoism_ui::panels::file_tree::ROW_HEIGHT*scale).abs()>f32::EPSILON{tree.set_scale(scale);}}
            let height = n(&v, "height", 600.0);
            let rows = tree.visible_rows_for_panel_height(height);
            match op {
                "tree.layout" => {
                    let entries = v["entries"]
                        .as_array()
                        .ok_or("Missing entries")?
                        .iter()
                        .map(|e| TreeEntry {
                            label: text(e, "name").to_owned(),
                            depth: n(e, "depth", 0.0) as u8,
                            kind: if e["dir"].as_bool().unwrap_or(false) {
                                NodeKind::Dir {
                                    open: e["open"].as_bool().unwrap_or(false),
                                }
                            } else {
                                NodeKind::File
                            },
                            path: Some(PathBuf::from(text(e, "path"))),
                            git_status: GitStatus::None,
                            virtual_kind: None,
                        })
                        .collect();
                    tree.host_set_entries(entries);
                }
                "tree.scroll" => tree.scroll_pixels(n(&v, "delta", 0.0), rows),
                "tree.drag-begin" => {
                    tree.begin_file_drag(n(&v,"index",0.0) as usize,n(&v,"x",0.0),n(&v,"y",0.0));
                }
                "tree.drag-update" => {
                    let open=tree.update_file_drag(n(&v,"x",0.0),n(&v,"y",0.0),v["hovered"].as_u64().map(|index|index as usize),v["inside"].as_bool().unwrap_or(false));
                    let drag=tree.host_drag_frame().map(|(live,source,target,root,wiggle)|json!({"live":live,"source":source,"target":target,"root":root,"wiggle":wiggle}));
                    return Ok(json!({"drag":drag,"open":open}));
                }
                "tree.drag-end" => {
                    use neoism_ui::panels::file_tree::FileDropOutcome;
                    return Ok(match tree.end_file_drag(){
                        FileDropOutcome::Click=>json!({"kind":"click"}),
                        FileDropOutcome::Cancel=>json!({"kind":"cancel"}),
                        FileDropOutcome::Move{source,dest_dir}=>json!({"kind":"move","source":source,"directory":dest_dir}),
                    });
                }
                "tree.drag-cancel" => tree.cancel_file_drag(),
                "tree.select" => {
                    tree.set_selected(n(&v, "index", 0.0) as usize);
                    tree.host_reveal_selection(height);
                }
                "tree.key" => {
                    let key = text(&v, "key");
                    let ctrl = v["ctrl"].as_bool().unwrap_or(false);
                    if !ctrl
                        && key.len() == 1
                        && key.chars().next().is_some_and(|c| c.is_ascii_digit())
                    {
                        tree.push_count_digit(key.parse().unwrap_or(0));
                    } else {
                        match key {
                            "j" | "down" => {
                                let count = tree.take_count();
                                tree.select_next_by(count);
                            }
                            "k" | "up" => {
                                let count = tree.take_count();
                                tree.select_prev_by(count);
                            }
                            "d" if ctrl => tree.select_next_by((rows / 2).max(1)),
                            "u" if ctrl => tree.select_prev_by((rows / 2).max(1)),
                            "g" => {
                                if tree.note_g() {
                                    tree.select_first();
                                    tree.host_reveal_selection(height);
                                }
                            }
                            "G" | "end" => {
                                if let Some(count) = tree.pending_count() {
                                    tree.goto_row(count);
                                } else {
                                    tree.select_last();
                                }
                            }
                            "home" => tree.select_first(),
                            "escape" => tree.clear_pending(),
                            _ => {}
                        }
                        if matches!(key, "j" | "down" | "k" | "up" | "G" | "end" | "home")
                            || ctrl && matches!(key, "d" | "u") {
                            tree.host_reveal_selection(height);
                        }
                    }
                }
                "tree.frame" => {}
                _ => return Err("Unknown tree operation".into()),
            }
            let frame = tree.host_frame(height);
            Ok(
                json!({"top":frame.top,"offset":frame.offset,"selected":frame.selected,"cursorOffset":frame.cursor_offset,"animating":frame.animating}),
            )
        }
        "editor.clone-view" => {
            let view = engines.editors.get(&id).ok_or("Unknown editor")?.clone_view();
            engines.next += 1;
            let new_id = engines.next;
            engines.editors.insert(new_id,view);
            Ok(json!({"id":new_id}))
        }
        "editor.sync" => {
            let source_id = v["sourceID"].as_u64().ok_or("Missing source editor")? as u32;
            let buffer = engines.editors.get(&source_id).ok_or("Unknown source editor")?.pane.buffer.clone();
            let target = engines.editors.get_mut(&id).ok_or("Unknown editor")?;
            target.presence.clear();
            if buffer.doc_history_bound(){
                let old=target.pane.buffer.lines.join("\n");let new=buffer.lines.join("\n");
                if let Some(delta)=neoism_ui::editor::markdown::doc_sync::diff_doc_texts(&old,&new){neoism_ui::editor::code::doc_sync::apply_remote_delta_to_buffer(&mut target.pane.buffer,&delta);}
            }
            target.pane.buffer.sync_document_from(&buffer);
            target.pane.highlight = Default::default();
            Ok(target.frame())
        }
        "editor.new" => {
            engines.next += 1;
            let id = engines.next;
            let mut pane =
                CodePane::new(PathBuf::from(text(&v, "path")), text(&v, "text"));
            pane.remote_source = true;
            pane.input_mode = if v["vim"].as_bool().unwrap_or(false) {
                CodeInputMode::Vim
            } else {
                CodeInputMode::Standard
            };
            pane.buffer.mode = if pane.input_mode == CodeInputMode::Vim {
                CodeMode::Normal
            } else {
                CodeMode::Insert
            };
            engines.editors.insert(
                id,
                Editor {
                    collaboration:None, peers:Vec::new(),peer_paint:Default::default(),
                    pane,
                    register: String::new(),
                    clipboard: None,
                    host_action: None,
                    width: 800.0,
                    height: 600.0,
                    cell: 8.4,
                    row: 21.0,
                    trail: neoism_ui::panels::trail_cursor::TrailCursor::new(),
                    last_frame: Instant::now(),
                    last_cursor: (0, 0),
                    lsp: Default::default(),
                    compact: v["compact"].as_bool().unwrap_or(false),
                    raster: Default::default(),
                    presence: Default::default(),
                },
            );
            Ok(json!({"id":id}))
        }
        "dispose" => {
            engines.trees.remove(&id);
            engines.editors.remove(&id);
            Ok(Value::Null)
        }
        "cursor" => {
            let now = n(&v, "seconds", 0.0);
            Ok(
                json!({"blink":neoism_ui::cursor_style::cursor_blink_visible(now),"rainbow":neoism_ui::cursor_style::rainbow_color_u8(now)}),
            )
        }
        op if op.starts_with("editor.") => {
            let e = engines.editors.get_mut(&id).ok_or("Unknown editor")?;
            if !matches!(op,"editor.frame"|"editor.cursor-frame"){e.presence.clear();}
            match op {
                "editor.cursor-frame" => return Ok(e.cursor_frame()),
                "editor.crdt.bind" => {
                    if let Some(binding)=e.collaboration.as_mut(){
                        let expected=neoism_ui::editor::crdt::presence_buffer_id_for_path(std::path::Path::new(text(&v,"path")));
                        if binding.binding.buffer_id()!=expected{return Err("Shared recovery belongs to a different document".into());}
                        binding.reconnect(&mut e.pane.buffer);
                    }else{e.collaboration=Some(editor_collaboration::EditorCollaboration::new(v["clientId"].as_u64().ok_or("Missing CRDT client id")?,text(&v,"path"),&e.pane.buffer));}
                    return Ok(e.collaboration.as_mut().unwrap().take(&mut e.pane.buffer));
                },
                "editor.crdt.flush" => return Ok(e.collaboration.as_mut().ok_or("Editor is not document-bound")?.take(&mut e.pane.buffer)),
                "editor.crdt.reconnect" => {e.collaboration.as_mut().ok_or("Editor is not document-bound")?.reconnect(&mut e.pane.buffer);return Ok(e.collaboration.as_mut().unwrap().take(&mut e.pane.buffer));},
                "editor.crdt.event" => {e.collaboration.as_mut().ok_or("Editor is not document-bound")?.event(serde_json::from_value(v["message"].clone()).map_err(|e|e.to_string())?,&mut e.pane.buffer)?;},
                "editor.crdt.history" => {e.collaboration.as_mut().ok_or("Editor is not document-bound")?.history(&mut e.pane.buffer,v["redo"].as_bool().unwrap_or(false));},
                "editor.crdt.take-history" => return Ok(json!(e.pane.buffer.take_doc_history_requests().into_iter().map(|r|matches!(r,neoism_ui::editor::code::CodeDocHistoryRequest::Redo)).collect::<Vec<_>>())),
                "editor.presence" => {e.peer_paint.configure(&v);e.peers=serde_json::from_value(v["peers"].clone()).map_err(|e|e.to_string())?;},
                "editor.snapshot" => return Ok(e.snapshot()),
                "editor.restore" => e.restore(&v["snapshot"] )?,
                "editor.raster.configure" => e.raster.configure(&v),
                "editor.raster.stats" => return Ok(e.raster.stats()),
                "editor.layout" => {
                    e.width = n(&v, "width", 800.0);
                    e.height = n(&v, "height", 600.0);
                    e.cell = n(&v, "cellWidth", 8.4);
                    e.row = n(&v, "rowHeight", 21.0);
                }
                "editor.wrap" => {
                    e.pane.wrap = v["enabled"].as_bool().unwrap_or(!e.pane.wrap);
                    e.pane.buffer.follow_cursor = true;
                },
                "editor.toggle-vim" => {
                    let vim = v["enabled"].as_bool().unwrap_or(e.pane.input_mode != CodeInputMode::Vim);
                    e.pane.input_mode = if vim { CodeInputMode::Vim } else { CodeInputMode::Standard };
                    e.pane.buffer.break_undo_group();
                    e.pane.buffer.clear_extra_carets();
                    e.pane.buffer.clear_selection();
                    e.pane.buffer.vim.clear_pending();
                    e.pane.buffer.mode = if vim { CodeMode::Normal } else { CodeMode::Insert };
                    if vim { e.pane.buffer.snap_normal_cursor(); }
                    e.lsp.session.completion = None;
                },
                "editor.path" => {
                    let path = text(&v, "path");
                    if path.is_empty() { return Err("Missing editor host path".into()); }
                    if e.pane.path != PathBuf::from(path) { e.collaboration = None; e.pane.buffer.set_doc_history_bound(false); }
                    e.pane.path = PathBuf::from(path);
                },
                "editor.lsp.enable" => {
                    if let Some(path) = v["path"].as_str() { e.pane.path = PathBuf::from(path); }
                    e.lsp.enable(text(&v,"surface").to_owned(),v["openPaths"].as_array().into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from).collect());
                },
                "editor.lsp.paths" => e.lsp.open_paths=v["openPaths"].as_array().into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from).collect(),
                "editor.lsp.disable" => e.lsp = Default::default(),
                "editor.lsp.reply" => e.lsp.reply(&mut e.pane,v["message"].clone())?,
                "editor.lsp.action" => e.lsp.action(&mut e.pane,&v)?,
                "editor.lsp.pointer" => {
                    let pos = v["x"].as_f64().zip(v["y"].as_f64()).map(|(x,y)|(x as f32,y as f32));
                    e.lsp.pointer(&e.pane,pos);
                },
                "editor.lsp.hover-configure" => e.lsp.hover_configure(&v),
                "editor.lsp.hover-input" => e.lsp.hover_input(&v)?,
                "editor.goto" => {
                    let line=v["line"].as_u64().unwrap_or(0) as usize;
                    let col=v["col"].as_u64().unwrap_or(0) as usize;
                    e.pane.buffer.set_cursor_position(line,col,false);
                    e.pane.buffer.follow_cursor=true;
                },
                "editor.lsp.geometry" => e.lsp.geometry(&v),
                "editor.lsp.wheel" => e.lsp.wheel(n(&v,"delta",0.0),v["precise"].as_bool().unwrap_or(false)),
                "editor.lsp.select" => e.lsp.select(&mut e.pane,v["index"].as_u64().unwrap_or(0) as usize),
                "editor.apply-edits" => {
                    let edits = editor_lsp::text_edits(&v["edits"])?;
                    e.pane.buffer.validate_text_edits(&edits)?;
                    e.pane.buffer.apply_text_edits(&edits);
                },
                "editor.key" => {
                    let before=e.pane.buffer.revision;
                    e.key(&v);
                    if e.pane.buffer.revision != before {
                        use neoism_ui::editor::code::lsp_session::LspKeyEdit as K;
                        let edit=if text(&v,"key")=="backspace" {K::Backspace}
                            else if !v["ctrl"].as_bool().unwrap_or(false) && text(&v,"char").chars().count()==1 {K::Char(text(&v,"char").chars().next().unwrap())}
                            else {K::Other};
                        if e.pane.buffer.mode==CodeMode::Insert { e.lsp.session.after_key(&e.pane,edit); }
                    }
                    if e.pane.buffer.mode!=CodeMode::Insert {e.lsp.session.completion=None;}
                    // Original key dispatch mutates the buffer then marks the
                    // view dirty. Pure held motion must not tick either spring
                    // or rebuild the paint tree between display frames. An
                    // operator motion may edit/yank: publish those normally.
                    if v["deferFrame"].as_bool().unwrap_or(false)
                        && matches!(text(&v,"key"),"up"|"down")
                        && e.pane.buffer.revision==before && e.clipboard.is_none() && e.host_action.is_none()
                    {
                        let selection=e.pane.buffer.selection_range();
                        return Ok(json!({"deferred":true,
                            "cursor":{"line":e.pane.buffer.cursor_line,"col":e.pane.buffer.cursor_col},
                            "mode":format!("{:?}",e.pane.buffer.mode),
                            "pendingEmpty":e.pane.buffer.vim.pending.is_empty()&&!e.pane.leader_pending,
                            "pendingKeys":e.pane.buffer.vim.pending.showcmd(),
                            "selection":selection.map(|(a,b)|json!({"start":[a.line,a.col],"end":[b.line,b.col]}))}));
                    }
                },
                "editor.scroll" => e.pane.scroll_pixels(n(&v, "delta", 0.0), e.height),
                "editor.pointer" => {
                    let (line, col) = e.pane.geometry.hit_position(
                        &e.pane.buffer.lines,
                        n(&v, "x", 0.0),
                        n(&v, "y", 0.0),
                    );
                    if v["drag"].as_bool().unwrap_or(false) {
                        e.pane.extend_pointer_selection_at(n(&v,"x",0.0),n(&v,"y",0.0));
                    } else {
                        e.pane.begin_pointer_selection(line, col,
                            v["clicks"].as_u64().unwrap_or(1) as usize,
                            v["extend"].as_bool().unwrap_or(false));
                    }
                }
                "editor.pointer-up" => e.pane.mouse_selecting = false,
                "editor.copy" | "editor.cut" => {
                    let (mut payload, linewise) = if op == "editor.cut" {
                        e.pane.buffer.cut_payload()
                    } else {
                        e.pane.buffer.copy_payload()
                    };
                    if linewise {
                        payload.push('\n');
                    }
                    if e.pane.input_mode == CodeInputMode::Vim
                        && e.pane.buffer.mode != CodeMode::Insert
                    {
                        e.pane.buffer.mode = CodeMode::Normal;
                        e.pane.buffer.clear_selection();
                        e.pane.buffer.snap_normal_cursor();
                    }
                    return Ok(json!({"text":payload}));
                }
                "editor.text" => {
                    return Ok(json!({"text":e.pane.buffer.text_for_disk()}))
                }
                "editor.search.start" => {
                    e.pane.search_origin =
                        Some((e.pane.buffer.cursor_line, e.pane.buffer.cursor_col));
                    e.pane.search_backward = v["backward"].as_bool().unwrap_or(false);
                }
                "editor.search.preview" => {
                    use neoism_ui::editor::markdown::{
                        vim::{vim_search_backward, vim_search_forward},
                        MarkdownPosition,
                    };
                    let pattern = text(&v, "pattern");
                    if let Some((line, col)) = e.pane.search_origin {
                        if pattern.is_empty() {
                            e.pane.buffer.set_cursor_position(line, col, false);
                            e.pane.search_highlight = None;
                        } else {
                            e.pane.search_highlight = Some(pattern.to_owned());
                            let found = if e.pane.search_backward {
                                vim_search_backward(
                                    &e.pane.buffer.lines,
                                    MarkdownPosition { line, col: col + 1 },
                                    pattern,
                                    false,
                                )
                            } else {
                                let start = if col > 0 {
                                    MarkdownPosition { line, col: col - 1 }
                                } else if line > 0 {
                                    MarkdownPosition {
                                        line: line - 1,
                                        col: usize::MAX,
                                    }
                                } else {
                                    MarkdownPosition { line: 0, col: 0 }
                                };
                                vim_search_forward(
                                    &e.pane.buffer.lines,
                                    start,
                                    pattern,
                                    false,
                                )
                            };
                            if let Some(found) = found {
                                e.pane
                                    .buffer
                                    .set_cursor_position(found.line, found.col, false);
                            }
                        }
                        e.pane.buffer.follow_cursor = true;
                    }
                }
                "editor.search.cancel" => {
                    if let Some((line, col)) = e.pane.search_origin.take() {
                        e.pane.buffer.set_cursor_position(line, col, false);
                    }
                    e.pane.search_highlight = None;
                }
                "editor.search.commit" => {
                    let pattern = text(&v, "pattern");
                    if !pattern.is_empty() {
                        e.pane.buffer.vim.search =
                            Some(neoism_ui::editor::markdown::vim::VimSearch {
                                pattern: pattern.to_owned(),
                                forward: !e.pane.search_backward,
                                whole_word: false,
                            });
                    }
                    e.pane.search_origin = None;
                }
                "editor.command" => {
                    use neoism_ui::editor::code::substitute::{
                        apply_substitute, parse_substitute_command,
                    };
                    use neoism_ui::editor::scroll_model::{
                        parse_ex_command, MarkdownExCommandPlan as Plan,
                    };
                    let command =
                        text(&v, "command").trim().trim_start_matches(':').trim();
                    let mut action = Value::Null;
                    if let Some(spec) = parse_substitute_command(command) {
                        apply_substitute(&mut e.pane.buffer, &spec);
                    } else if let Some((head, _tail)) = parse_ex_command(command) {
                        match Plan::classify(&head) {
                            Plan::JumpToLastLine => {
                                let last = e.pane.buffer.line_count().saturating_sub(1);
                                e.pane.buffer.set_cursor_position(last, 0, false);
                            }
                            Plan::JumpToLine(line) => {
                                let line = line
                                    .saturating_sub(1)
                                    .min(e.pane.buffer.line_count().saturating_sub(1));
                                e.pane.buffer.set_cursor_position(line, 0, false);
                            }
                            Plan::Save => action = json!("save"),
                            Plan::SaveAndCloseFocusedBuffer => {
                                action = json!("save-close")
                            }
                            _ if head == "q" || head == "quit" => action = json!("close"),
                            _ if head == "noh" || head == "nohlsearch" => {
                                e.pane.search_highlight = None
                            }
                            _ => {
                                return Err(format!(
                                    "Command is not ported yet: :{command}"
                                ))
                            }
                        }
                    }
                    e.pane.buffer.follow_cursor = true;
                    let mut frame = e.frame();
                    frame["hostAction"] = action;
                    return Ok(frame);
                }
                "editor.saved" => {
                    e.lsp.session.notify_saved(&e.pane.path);
                    if v["revision"]
                        .as_u64()
                        .map_or(true, |revision| revision == e.pane.buffer.revision)
                    {
                        e.pane.buffer.mark_saved();
                    }
                }
                "editor.replace-source" => {
                    if e.pane.buffer.doc_history_bound(){
                        if let Some(delta)=neoism_ui::editor::markdown::doc_sync::diff_doc_texts(&e.pane.buffer.lines.join("\n"),text(&v,"text")){neoism_ui::editor::code::doc_sync::apply_remote_delta_to_buffer(&mut e.pane.buffer,&delta);}
                    }else{e.pane.buffer.replace_from_disk_text(text(&v, "text"));}
                }
                "editor.paste" => e.pane.buffer.insert_text(text(&v, "text")),
                "editor.frame" => {}
                _ => return Err("Unknown editor operation".into()),
            }
            if v["overlayOnly"].as_bool().unwrap_or(false) && matches!(op,"editor.lsp.pointer"|"editor.lsp.geometry"|"editor.lsp.hover-configure"|"editor.lsp.hover-input"|"editor.lsp.wheel") {
                return Ok(json!({"lspOnly":true,"lsp":e.lsp.frame(&e.pane)}));
            }
            Ok(e.frame())
        }
        _ => Err("Unknown engine operation".into()),
    }
}

#[no_mangle]
pub unsafe extern "C" fn neoism_core_call(bytes: *const u8, len: usize) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let input = std::slice::from_raw_parts(bytes, len);
        let value: Value = serde_json::from_slice(input).map_err(|e| e.to_string())?;
        if let Some(response) = agent_timeline::serialized_frame(&value) { return response; }
        ENGINES.with(|engines| dispatch(value, &mut engines.borrow_mut()))
            .map(|value| json!({"ok": value}).to_string())
    }));
    let output = match result {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => json!({"error":error}).to_string(),
        Err(_) => json!({"error":"Native engine failed"}).to_string(),
    };
    CString::new(output).unwrap().into_raw()
}
#[no_mangle]
pub unsafe extern "C" fn neoism_core_free(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
}
