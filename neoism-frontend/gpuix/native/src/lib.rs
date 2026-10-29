//! Data-only host bridge. The existing Rust engines own behavior; GPUIX paints.
//! No Sugarloaf renderer, OS window, PTY process, or filesystem I/O is created here.
use neoism_terminal_core::ansi::CursorShape;
use neoism_terminal_core::crosswords::{
    grid::Scroll,
    pos::{Column, Line, Pos, Side},
    Mode,
};
use neoism_terminal_core::handler::Processor;
use neoism_terminal_core::{
    Crosswords, Selection, SelectionType, TerminalEffect, TerminalId,
};
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
#[allow(dead_code)]
#[path = "../../../desktop/src/input/kitty_keyboard.rs"]
mod kitty_keyboard;
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
mod finder;
mod command_catalog;
mod chrome;
mod presence;
mod keyboard_policy;
mod tree_glyphs;
mod menu_actions;
mod server_management;
mod service_process;
#[path = "../../../desktop/src/fast_servers.rs"]
mod fast_servers;
mod agent_composer;
mod splash;
mod wordmark;
mod breadcrumbs;
mod git_panel;
mod git_review;
mod git_confirm;

fn terminal_key(v: &Value, mode: Mode) -> Vec<u8> {
    use neoism_ui::selection_input::{
        should_build_key_sequence, KeySequenceShapeInput, OutputLogicalKey as O,
    };
    use neoism_window::{
        event::ElementState,
        keyboard::{Key, KeyLocation, ModifiersState, NamedKey},
    };
    let key = match text(v, "key") {
        "space" => " ",
        key => key,
    };
    let typed = match text(v, "char") {
        "" if key == " " => " ",
        typed => typed,
    };
    let named = match key {
        "enter" | "return" => Some(NamedKey::Enter),
        "tab" => Some(NamedKey::Tab),
        "escape" => Some(NamedKey::Escape),
        "backspace" => Some(NamedKey::Backspace),
        "delete" => Some(NamedKey::Delete),
        "up" => Some(NamedKey::ArrowUp),
        "down" => Some(NamedKey::ArrowDown),
        "left" => Some(NamedKey::ArrowLeft),
        "right" => Some(NamedKey::ArrowRight),
        "home" => Some(NamedKey::Home),
        "end" => Some(NamedKey::End),
        "pageup" => Some(NamedKey::PageUp),
        "pagedown" => Some(NamedKey::PageDown),
        "f1" => Some(NamedKey::F1),
        "f2" => Some(NamedKey::F2),
        "f3" => Some(NamedKey::F3),
        "f4" => Some(NamedKey::F4),
        "f5" => Some(NamedKey::F5),
        "f6" => Some(NamedKey::F6),
        "f7" => Some(NamedKey::F7),
        "f8" => Some(NamedKey::F8),
        "f9" => Some(NamedKey::F9),
        "f10" => Some(NamedKey::F10),
        "f11" => Some(NamedKey::F11),
        "f12" => Some(NamedKey::F12),
        _ => None,
    };
    let mut mods = ModifiersState::empty();
    for (field, flag) in [
        ("ctrl", ModifiersState::CONTROL),
        ("shift", ModifiersState::SHIFT),
        ("alt", ModifiersState::ALT),
        ("cmd", ModifiersState::SUPER),
    ] {
        mods.set(flag, v[field].as_bool().unwrap_or(false));
    }
    let released = v["released"].as_bool().unwrap_or(false);
    if released && !mode.contains(Mode::REPORT_EVENT_TYPES) {
        return vec![];
    }
    // The desktop's default bindings run before its protocol encoder.
    // Preserve their legacy SS3 cursor/function-key sequences here too.
    if mods.is_empty()
        && !mode.intersects(
            Mode::REPORT_ALL_KEYS_AS_ESC
                | Mode::DISAMBIGUATE_ESC_CODES
                | Mode::REPORT_EVENT_TYPES,
        )
    {
        let suffix = match key {
            "up" if mode.contains(Mode::APP_CURSOR) => Some('A'),
            "down" if mode.contains(Mode::APP_CURSOR) => Some('B'),
            "right" if mode.contains(Mode::APP_CURSOR) => Some('C'),
            "left" if mode.contains(Mode::APP_CURSOR) => Some('D'),
            "home" if mode.contains(Mode::APP_CURSOR) => Some('H'),
            "end" if mode.contains(Mode::APP_CURSOR) => Some('F'),
            "f1" => Some('P'),
            "f2" => Some('Q'),
            "f3" => Some('R'),
            "f4" => Some('S'),
            _ => None,
        };
        if let Some(suffix) = suffix {
            return format!("\x1bO{suffix}").into_bytes();
        }
    }
    let logical = named.map(Key::Named).unwrap_or_else(|| {
        Key::Character(if typed.is_empty() { key } else { typed }.into())
    });
    let logical_kind = match named {
        Some(NamedKey::Enter) => O::Enter,
        Some(NamedKey::Tab) => O::Tab,
        Some(NamedKey::Backspace) => O::Backspace,
        Some(NamedKey::Escape) => O::Escape,
        Some(_) => O::NamedWithoutText,
        None => O::NonNamed,
    };
    let raw = match named {
        Some(NamedKey::Enter) => "\r",
        Some(NamedKey::Tab) => "\t",
        Some(NamedKey::Backspace) => "\x7f",
        Some(NamedKey::Escape) => "\x1b",
        _ => typed,
    };
    let event = kitty_keyboard::ProtocolKeyEvent {
        logical_key: logical,
        location: KeyLocation::Standard,
        state: if released {
            ElementState::Released
        } else {
            ElementState::Pressed
        },
        repeat: v["repeat"].as_bool().unwrap_or(false),
        text: (!raw.is_empty()).then(|| raw.to_string()),
        unmodified_key: Key::Character(key.into()),
    };
    if should_build_key_sequence(KeySequenceShapeInput {
        key: logical_kind,
        key_on_numpad: false,
        text_empty: raw.is_empty(),
        mods_empty: mods.is_empty(),
        mods_shift_only: mods == ModifiersState::SHIFT,
        report_all_keys_as_esc: mode.contains(Mode::REPORT_ALL_KEYS_AS_ESC),
        disambiguate_esc_codes: mode.contains(Mode::DISAMBIGUATE_ESC_CODES),
    }) {
        return kitty_keyboard::build_protocol_key_sequence(&event, mods, mode);
    }
    let mut output = if mods.contains(ModifiersState::ALT) {
        vec![0x1b]
    } else {
        vec![]
    };
    if mods.contains(ModifiersState::CONTROL) && key.len() == 1 {
        let byte = key.as_bytes()[0].to_ascii_uppercase();
        if (b'@'..=b'_').contains(&byte) {
            output.push(byte - 64);
            return output;
        }
        if key == " " {
            output.push(0);
            return output;
        }
    }
    output.extend(raw.as_bytes());
    output
}

struct Terminal {
    inner: Crosswords,
    parser: Processor,
    scroll: neoism_ui::terminal_scroll_state::TerminalScroll,
    trail: neoism_ui::panels::trail_cursor::TrailCursor,
    last_frame: Instant,
    last_cursor: (usize, i32),
    search_pattern: String,
    search_forward: bool,
    search_origin: Option<Pos>,
    search_preview: Option<String>,
    search_preview_forward: bool,
}
impl Terminal {
    fn vi_key(&mut self, v: &Value) -> Option<Value> {
        use neoism_terminal_core::crosswords::vi_mode::ViMotion as M;
        use neoism_ui::editor::selection_model::{
            toggle_selection_action, SelectionSnapshot, ToggleSelectionAction as A,
        };
        let key = text(v, "key");
        let ch = text(v, "char");
        let ctrl = v["ctrl"].as_bool().unwrap_or(false);
        let shift = v["shift"].as_bool().unwrap_or(false);
        let alt = v["alt"].as_bool().unwrap_or(false);
        if v["released"].as_bool().unwrap_or(false) {
            return self
                .inner
                .mode()
                .contains(Mode::VI)
                .then(|| json!({"bytes":[],"handled":true}));
        }
        if alt && shift && (key == "space" || key == " ") {
            self.inner.toggle_vi_mode();
            self.scroll.reset_wheel(0);
            return Some(json!({"bytes":[],"handled":true}));
        }
        if !self.inner.mode().contains(Mode::VI) {
            return None;
        }
        let typed = if ch.is_empty() { key } else { ch };
        let mut clipboard = Value::Null;
        if key == "i" && !ctrl {
            self.inner.toggle_vi_mode();
            self.inner.scroll_display(Scroll::Bottom);
        } else if key == "c" && ctrl {
            self.inner.toggle_vi_mode();
        } else if key == "escape" {
            self.inner.selection = None;
        } else if key == "y" && !ctrl {
            clipboard = json!(self.inner.selection_to_string());
            self.inner.selection = None;
        } else if key == "v" || typed == "v" || typed == "V" {
            let ty = if ctrl {
                SelectionType::Block
            } else if alt {
                SelectionType::Semantic
            } else if shift || typed == "V" {
                SelectionType::Lines
            } else {
                SelectionType::Simple
            };
            let current = self.inner.selection.as_ref().map(|s| SelectionSnapshot {
                ty: s.ty,
                is_empty: s.is_empty(),
            });
            match toggle_selection_action(current, ty) {
                A::Clear => self.inner.selection = None,
                A::RetypeExisting => {
                    if let Some(s) = self.inner.selection.as_mut() {
                        s.ty = ty;
                        s.include_all();
                    }
                }
                A::StartAtCursor => {
                    let mut selection =
                        Selection::new(ty, self.inner.vi_mode_cursor.pos, Side::Left);
                    selection.include_all();
                    self.inner.selection = Some(selection);
                }
            }
        } else if ctrl && matches!(key, "u" | "d" | "b" | "f" | "y" | "e") {
            let magnitude = match key {
                "b" | "f" => self.inner.screen_lines() as i32,
                "y" | "e" => 1,
                _ => (self.inner.screen_lines() as i32 / 2).max(1),
            };
            let delta = if matches!(key, "d" | "f" | "e") {
                -magnitude
            } else {
                magnitude
            };
            self.inner.vi_mode_cursor =
                self.inner.vi_mode_cursor.scroll(&self.inner, delta);
            self.inner.scroll_display(Scroll::Delta(delta));
        } else if !ctrl && matches!(typed, "n" | "N") {
            use neoism_terminal_core::crosswords::{
                pos::{Boundary, Direction},
                search::RegexSearch,
            };
            if let Ok(mut regex) = RegexSearch::new(&self.search_pattern) {
                let forward = self.search_forward != (typed == "N");
                let origin = if forward {
                    self.inner
                        .vi_mode_cursor
                        .pos
                        .add(&self.inner, Boundary::Grid, 1)
                } else {
                    self.inner
                        .vi_mode_cursor
                        .pos
                        .sub(&self.inner, Boundary::Grid, 1)
                };
                let direction = if forward {
                    Direction::Right
                } else {
                    Direction::Left
                };
                if let Some(found) = self.inner.search_next(
                    &mut regex,
                    origin,
                    direction,
                    Side::Left,
                    None,
                ) {
                    self.inner.vi_goto_pos(*found.start());
                }
            }
        } else if !ctrl {
            match typed {
                "g" => self.inner.scroll_display(Scroll::Top),
                "G" => self.inner.scroll_display(Scroll::Bottom),
                "z" => {
                    let target = -(self.inner.grid.display_offset() as i32)
                        + self.inner.screen_lines() as i32 / 2
                        - 1;
                    self.inner.scroll_display(Scroll::Delta(
                        target - self.inner.vi_mode_cursor.pos.row.0,
                    ));
                }
                _ => {
                    let motion = match typed {
                        "h" | "left" => Some(M::Left),
                        "j" | "down" => Some(M::Down),
                        "k" | "up" => Some(M::Up),
                        "l" | "right" => Some(M::Right),
                        "0" | "home" => Some(M::First),
                        "$" | "end" => Some(M::Last),
                        "^" => Some(M::FirstOccupied),
                        "H" => Some(M::High),
                        "M" => Some(M::Middle),
                        "L" => Some(M::Low),
                        "b" => Some(M::SemanticLeft),
                        "w" => Some(M::SemanticRight),
                        "e" => Some(M::SemanticRightEnd),
                        "B" => Some(M::WordLeft),
                        "W" => Some(M::WordRight),
                        "E" => Some(M::WordRightEnd),
                        "%" => Some(M::Bracket),
                        _ => None,
                    };
                    if let Some(motion) = motion {
                        self.inner.vi_motion(motion);
                    }
                }
            }
        }
        Some(json!({"bytes":[],"handled":true,"clipboard":clipboard}))
    }
}
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
}
#[derive(Default)]
struct Engines {
    next: u32,
    trees: HashMap<u32, FileTree>,
    terminals: HashMap<u32, Terminal>,
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
            last_frame: Instant::now(), last_cursor: self.last_cursor, lsp: Default::default(), compact: self.compact, raster: self.raster.clone_settings() }
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
        self.register = text(snapshot,"register").to_owned();
        self.clipboard = None;
        self.host_action = None;
        self.trail = neoism_ui::panels::trail_cursor::TrailCursor::new();
        Ok(())
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
        json!({"peerCommands":self.peer_paint.frame(pane,&self.peers),"remotePeers":self.peers,"collaborationBound":pane.buffer.doc_history_bound(),"presence":editor_collaboration::cursor(&pane.buffer),"packedRuns":self.compact,"lsp":lsp,"lines":lines,"searchHits":search_hits,"extraCarets":extra_carets,"rowHeight":row,"cellWidth":cell,"textX":text_x,"gutterWidth":gutter,
            "cursor":{"x":cursor_x,"y":cursor_y,"line":pane.buffer.cursor_line,"col":pane.buffer.cursor_col,
              "shape":format!("{:?}",pane.cursor_shape()),"visible":cursor_y+row>0.0 && cursor_y<self.height},
            "mode":format!("{:?}",pane.buffer.mode),"wrap":pane.wrap,"vim":pane.input_mode==CodeInputMode::Vim,"dirty":pane.buffer.is_dirty(),"revision":pane.buffer.revision,
            "totalLines":pane.buffer.line_count(),"pendingKeys":pane.buffer.vim.pending.showcmd(),
            "scrollY":pane.scroll_y,"contentHeight":view.content_h,"animating":view.scroll_animating||self.trail.is_animating(),
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
    if let Some(result) = ssh_transport::dispatch(&v) { return result; }
    if let Some(result) = presence::dispatch(&v) { return result; }
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
            let mut tree = FileTree::new(PathBuf::new());
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
        "terminal.new" => {
            engines.next += 1;
            let id = engines.next;
            engines.terminals.insert(
                id,
                Terminal {
                    inner: Crosswords::new(
                        (n(&v, "rows", 30.0) as usize, n(&v, "cols", 100.0) as usize),
                        CursorShape::Block,
                        TerminalId::new(id as u64),
                        2000,
                    ),
                    parser: Processor::new(),
                    scroll: Default::default(),
                    trail: neoism_ui::panels::trail_cursor::TrailCursor::new(),
                    last_frame: Instant::now(),
                    last_cursor: (0, 0),
                    search_pattern: String::new(),
                    search_forward: true,
                    search_origin: None,
                    search_preview: None,
                    search_preview_forward: true,
                },
            );
            Ok(json!({"id":id}))
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
                },
            );
            Ok(json!({"id":id}))
        }
        "dispose" => {
            engines.trees.remove(&id);
            engines.terminals.remove(&id);
            engines.editors.remove(&id);
            Ok(Value::Null)
        }
        "cursor" => {
            let now = n(&v, "seconds", 0.0);
            Ok(
                json!({"blink":neoism_ui::cursor_style::cursor_blink_visible(now),"rainbow":neoism_ui::cursor_style::rainbow_color_u8(now)}),
            )
        }
        op if op.starts_with("terminal.") => {
            let t = engines.terminals.get_mut(&id).ok_or("Unknown terminal")?;
            if op == "terminal.key" {
                if let Some(reply) = t.vi_key(&v) {
                    return Ok(reply);
                }
                return Ok(
                    json!({"bytes":terminal_key(&v,t.inner.mode()),"handled":false}),
                );
            }
            if op == "terminal.focus" {
                return Ok(
                    json!({"bytes":if t.inner.mode().contains(Mode::FOCUS_IN_OUT){if v["focused"].as_bool().unwrap_or(false){b"\x1b[I".to_vec()}else{b"\x1b[O".to_vec()}}else{vec![]}}),
                );
            }
            if op == "terminal.mouse" {
                use neoism_ui::mouse_policy::{
                    encode_normal_mouse_report, encode_sgr_mouse_report,
                    mouse_report_legacy_button_byte, mouse_report_modifier_bits,
                };
                let mode = t.inner.mode();
                if !mode.intersects(Mode::MOUSE_MODE)
                    || mode.contains(Mode::VI)
                    || v["shift"].as_bool().unwrap_or(false)
                {
                    return Ok(json!({"handled":false,"bytes":[]}));
                }
                let row = n(&v, "row", 0.0) as i32 - t.inner.grid.display_offset() as i32;
                if row < 0 {
                    return Ok(json!({"handled":true,"bytes":[]}));
                }
                let pos = Pos::new(
                    Line(row.min(t.inner.screen_lines() as i32 - 1)),
                    Column(
                        (n(&v, "col", 0.0).max(0.0) as usize).min(t.inner.columns() - 1),
                    ),
                );
                let kind = text(&v, "kind");
                let motion = kind == "move";
                let pressed = kind != "up";
                let button = v["button"].as_u64().map(|n| n as u8);
                if motion
                    && !mode.contains(Mode::MOUSE_MOTION)
                    && (!mode.contains(Mode::MOUSE_DRAG) || button.is_none())
                {
                    return Ok(json!({"handled":true,"bytes":[]}));
                }
                let mods = mouse_report_modifier_bits(
                    false,
                    v["alt"].as_bool().unwrap_or(false),
                    v["ctrl"].as_bool().unwrap_or(false),
                );
                let mut buttons = Vec::new();
                if kind == "wheel" {
                    let height = n(&v, "rowHeight", 21.0).max(1.0);
                    let delta = n(&v, "delta", 0.0)
                        * if v["precise"].as_bool().unwrap_or(false) {
                            1.0
                        } else {
                            height * 3.0
                        };
                    let steps =
                        t.scroll.add_wheel_delta(1, delta, height).clamp(-200, 200);
                    for _ in 0..steps.unsigned_abs() {
                        buttons.push(if steps > 0 { 64 } else { 65 });
                    }
                } else {
                    buttons.push(button.unwrap_or(3) + if motion { 32 } else { 0 });
                }
                let mut bytes = Vec::new();
                for button in buttons {
                    if mode.contains(Mode::SGR_MOUSE) {
                        bytes.extend(encode_sgr_mouse_report(
                            pos,
                            button + mods,
                            pressed,
                        ));
                    } else if let Some(report) = encode_normal_mouse_report(
                        pos,
                        mouse_report_legacy_button_byte(button, mods, pressed),
                        mode.contains(Mode::UTF8_MOUSE),
                    ) {
                        bytes.extend(report);
                    }
                }
                return Ok(json!({"handled":true,"bytes":bytes}));
            }
            if op == "terminal.cursor" {
                let cell = n(&v, "cellWidth", 8.4);
                let row = n(&v, "rowHeight", 21.0);
                let pos = if t.inner.mode().contains(Mode::VI) {
                    t.inner.vi_mode_cursor.pos
                } else {
                    t.inner.grid.cursor.pos
                };
                let position = (pos.col.0, pos.row.0);
                let x = pos.col.0 as f32 * cell;
                let y = (pos.row.0 + t.inner.grid.display_offset() as i32) as f32 * row
                    + t.scroll.current_offset(0);
                let shape = match text(&v, "shape") {
                    "Beam" => CursorShape::Beam,
                    "Underline" => CursorShape::Underline,
                    "Hidden" => CursorShape::Hidden,
                    _ => CursorShape::Block,
                };
                let was_animating = t.trail.is_animating();
                t.trail.set_cursor_shape(shape);
                if position != t.last_cursor {
                    t.trail.set_destination(x, y, cell, row);
                } else {
                    t.trail.set_destination_no_jump(x, y, cell, row);
                }
                t.last_cursor = position;
                let now = Instant::now();
                let dt = now.duration_since(t.last_frame).as_secs_f32();
                t.last_frame = now;
                t.trail.animate(cell, row, if was_animating { dt } else { 0.0 });
                return Ok(
                    json!({"quad":t.trail.quad_points(),"animating":t.trail.is_animating(),"hold":t.trail.blink_hold_visible(neoism_ui::editor::optimistic::DEFAULT_BLINK_PERIOD_MS)}),
                );
            }
            match op {
                "terminal.feed" => {
                    let bytes: Vec<u8> = serde_json::from_value(v["bytes"].clone())
                        .map_err(|e| e.to_string())?;
                    t.parser.advance(&mut t.inner, &bytes);
                }
                "terminal.reset" => {
                    let rows = t.inner.screen_lines();
                    let cols = t.inner.columns();
                    t.inner = Crosswords::new(
                        (rows, cols),
                        CursorShape::Block,
                        TerminalId::new(id as u64),
                        2000,
                    );
                    t.parser = Processor::new();
                    t.scroll = Default::default();
                }
                "terminal.resize" => t.inner.resize((
                    n(&v, "rows", 30.0).clamp(1.0, 200.0) as usize,
                    n(&v, "cols", 100.0).clamp(2.0, 500.0) as usize,
                )),
                "terminal.scroll" => {
                    let row = n(&v, "rowHeight", 21.0);
                    let before = t.inner.snapshot().display_offset;
                    let steps = t.scroll.add_wheel_delta(0, n(&v, "delta", 0.0), row);
                    t.inner.scroll_display(Scroll::Delta(steps));
                    if steps != 0 && before == t.inner.snapshot().display_offset {
                        t.scroll.reset_wheel(0);
                    }
                }
                "terminal.bottom" => {
                    t.inner.scroll_display(Scroll::Bottom);
                    t.scroll.reset_wheel(0);
                    t.inner.selection = None;
                }
                "terminal.select" => {
                    let point = Pos::new(
                        Line(
                            n(&v, "row", 0.0) as i32
                                - t.inner.snapshot().display_offset as i32,
                        ),
                        Column(n(&v, "col", 0.0) as usize),
                    );
                    let side = neoism_ui::mouse_policy::cell_side_by_pos(
                        n(&v, "x", 0.0).max(0.0) as usize,
                        0.0,
                        n(&v, "cellWidth", 1.0).max(1.0),
                        t.inner.columns() as f32 * n(&v, "cellWidth", 1.0).max(1.0),
                    );
                    if v["start"].as_bool().unwrap_or(false) {
                        let kind = match v["clicks"].as_u64().unwrap_or(1) {
                            2 => SelectionType::Semantic,
                            3.. => SelectionType::Lines,
                            _ => SelectionType::Simple,
                        };
                        t.inner.selection = Some(Selection::new(kind, point, side));
                    } else if let Some(selection) = t.inner.selection.as_mut() {
                        selection.update(point, side);
                    }
                }
                "terminal.search.start" => {
                    t.search_origin = Some(t.inner.vi_mode_cursor.pos);
                    t.search_preview = Some(String::new());
                    t.search_preview_forward = v["forward"].as_bool().unwrap_or(true);
                }
                "terminal.search.preview" => {
                    use neoism_terminal_core::crosswords::{
                        pos::Direction, search::RegexSearch,
                    };
                    let pattern = text(&v, "pattern");
                    t.search_preview = Some(pattern.to_owned());
                    if let Some(origin) = t.search_origin {
                        if pattern.is_empty() {
                            t.inner.vi_goto_pos(origin);
                        } else {
                            let mut regex =
                                RegexSearch::new(pattern).map_err(|e| e.to_string())?;
                            let direction = if t.search_preview_forward {
                                Direction::Right
                            } else {
                                Direction::Left
                            };
                            if let Some(found) = t.inner.search_next(
                                &mut regex,
                                origin,
                                direction,
                                Side::Left,
                                None,
                            ) {
                                t.inner.vi_goto_pos(*found.start());
                            }
                        }
                    }
                }
                "terminal.search.commit" => {
                    if let Some(pattern) = t.search_preview.take() {
                        t.search_pattern = pattern;
                        t.search_forward = t.search_preview_forward;
                    }
                    t.search_origin = None;
                }
                "terminal.search.cancel" => {
                    if let Some(origin) = t.search_origin.take() {
                        t.inner.vi_goto_pos(origin);
                    }
                    t.search_preview = None;
                }
                "terminal.frame" => {}
                _ => return Err("Unknown terminal operation".into()),
            }
            let mut writes = Vec::new();
            for effect in t.inner.drain_effects() {
                if let TerminalEffect::PtyWrite(bytes) = effect {
                    writes.extend(bytes);
                }
            }
            let mut snapshot = t.inner.snapshot();
            if t.inner.mode().contains(Mode::VI) {
                let pos = t.inner.vi_mode_cursor.pos;
                let row = pos.row.0 + snapshot.display_offset as i32;
                snapshot.cursor.col = pos.col.0 as u16;
                snapshot.cursor.row = row.max(0) as u16;
                snapshot.cursor.shape = neoism_terminal_core::CursorShape::Block;
                snapshot.cursor.visible = row >= 0 && row < (snapshot.rows as i32);
            }
            let mut search_matches = Vec::new();
            let pattern = t.search_preview.as_deref().unwrap_or(&t.search_pattern);
            if !pattern.is_empty() {
                use neoism_terminal_core::crosswords::{
                    pos::Direction,
                    search::{RegexIter, RegexSearch},
                };
                if let Ok(mut regex) = RegexSearch::new(pattern) {
                    let offset = snapshot.display_offset as i32;
                    let start = Pos::new(Line(-offset), Column(0));
                    let end = Pos::new(
                        Line(snapshot.rows as i32 - 1 - offset),
                        Column(snapshot.cols as usize - 1),
                    );
                    for range in
                        RegexIter::new(start, end, Direction::Right, &t.inner, &mut regex)
                            .take(500)
                    {
                        let a = range.start();
                        let b = range.end();
                        search_matches.push(json!({"start":[a.row.0+offset,a.col.0],"end":[b.row.0+offset,b.col.0]}));
                    }
                }
            }
            let mut graphemes = serde_json::Map::new();
            for row in 0..snapshot.rows as i32 {
                for col in 0..snapshot.cols as usize {
                    let cell = &t.inner.grid[Pos::new(
                        Line(row - snapshot.display_offset as i32),
                        Column(col),
                    )];
                    if let Some(extra) = cell
                        .extras_id()
                        .and_then(|id| t.inner.grid.extras_table.get(id))
                    {
                        if !extra.zerowidth.is_empty() {
                            let mut content = cell.c().to_string();
                            content.extend(extra.zerowidth.iter());
                            graphemes.insert(format!("{row}:{col}"), json!(content));
                        }
                    }
                }
            }
            let selection=t.inner.selection.as_ref().and_then(|s|s.to_range(&t.inner)).map(|r|json!({"start":[r.start.row.0+snapshot.display_offset as i32,r.start.col.0],"end":[r.end.row.0+snapshot.display_offset as i32,r.end.col.0]}));
            Ok(
                json!({"snapshot":snapshot,"searchMatches":search_matches,"graphemes":graphemes,"writes":writes,"offset":t.scroll.current_offset(0),"appCursor":t.inner.mode().contains(Mode::APP_CURSOR),"bracketedPaste":t.inner.mode().contains(Mode::BRACKETED_PASTE),"viMode":t.inner.mode().contains(Mode::VI),"selectionBlock":t.inner.selection.as_ref().is_some_and(|s|s.ty==SelectionType::Block),"selection":t.inner.selection_to_string(),"selectionRange":selection}),
            )
        }
        op if op.starts_with("editor.") => {
            let e = engines.editors.get_mut(&id).ok_or("Unknown editor")?;
            match op {
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
