//! Paint data for native hosts that share Finder policy without Sugarloaf.
use super::{modes::FinderMode, state::*, types::Result_, Finder};
use serde_json::{json, Value};
use web_time::Instant;

fn mode_name(mode: FinderMode) -> &'static str {
    match mode {
        FinderMode::Files => "files",
        FinderMode::Grep => "grep",
        FinderMode::GitChanges => "git",
        FinderMode::BufferLines => "buffer",
        FinderMode::BufferReplace => "replace",
        FinderMode::References => "references",
        FinderMode::Symbols => "symbols",
    }
}
impl Finder {
    /// Explicit remote preview installation; no local filesystem lookup.
    pub fn host_set_preview(&mut self, path: String, text: Option<String>) {
        if self.results.get(self.selected_index).map(|(_, r)| r.path())
            != Some(path.as_str())
        {
            return;
        }
        self.preview_content_path = Some(path);
        self.preview_content_unreadable = text.is_none();
        self.preview_content_lines = text
            .map(|t| t.lines().map(ToString::to_string).collect())
            .unwrap_or_default();
    }
    pub fn host_frame(&mut self, width: f32, height: f32) -> Value {
        let mut frame = self.host_frame_with_rows(width, height, true);
        // Legacy immediate hosts still paint their own blinking input caret.
        let caret_tick = CARET_BLINK_MS - Instant::now()
            .saturating_duration_since(self.caret_blink_start).as_millis() % CARET_BLINK_MS;
        frame["nextTickMs"] = json!(frame["nextTickMs"].as_u64().unwrap_or(caret_tick as u64).min(caret_tick as u64));
        frame
    }

    /// Compact host update: omitted rows mean retain the previous result set,
    /// whereas an explicit empty array clears it. Geometry never needs rows JSON.
    pub fn host_frame_with_rows(&mut self, width: f32, height: f32, include_rows: bool) -> Value {
        let scale = self.scale;
        let pad = FINDER_PADDING * scale;
        let input_h = INPUT_HEIGHT * scale;
        let row_h = RESULT_ITEM_HEIGHT * scale;
        let max_rows = self.max_visible_rows(scale);
        let visible_actual = self.results.len().min(max_rows).max(1);
        let preview_active = self.preview_enabled() && !self.results.is_empty();
        let body_h = row_h * visible_actual as f32 + 4.0 * scale;
        let body_h = if preview_active {
            (FINDER_HEIGHT * 0.55 * scale).max(body_h)
        } else {
            body_h
        };
        let base_h = (pad * 2.0 + input_h + SEPARATOR_HEIGHT + body_h)
            .min((height - 16.0).max(1.0))
            .max(1.0);
        let base_w = (self.overlay_width() * scale).min((width - 16.0).max(1.0));
        let (pop, pop_y) = self.open_pop_transform(scale);
        let w = base_w * pop;
        let h = base_h * pop;
        let x = (width - base_w) / 2.0 + (base_w - w) / 2.0;
        let y = self.top_anchor.min((height - base_h - 8.0).max(8.0)) + pop_y;
        let stroke = (crate::panels::file_tree::FRAME_STROKE * scale).max(2.0);
        let inner_x = stroke + pad;
        let inner_y = stroke + pad;
        let inner_w = (w - 2.0 * (stroke + pad)).max(0.0);
        let inner_h = (h - 2.0 * (stroke + pad)).max(0.0);
        let left_w = if preview_active {
            (inner_w * LEFT_COL_RATIO).floor()
        } else {
            inner_w
        };
        let results_y = inner_y + input_h + SEPARATOR_HEIGHT + 4.0 * scale;
        let list_h = (inner_y + inner_h - results_y).max(0.0);
        let visible_rows = (list_h / row_h).floor().max(1.0) as usize;
        self.clamp_scroll(visible_rows);
        let offset = self.tick_list_scroll();
        let cursor_offset = self.tick_cursor();
        let selected_y = (self.selected_index as i64 - self.scroll_offset as i64) as f32
            * row_h
            + offset
            + cursor_offset;
        let rows = include_rows.then(|| self.results.iter().enumerate().map(|(index,(score,r))| {
            let (text,marker,column,symbol)=match r {
                Result_::File(_)=>("".to_owned(),"",None,None),
                Result_::Grep(g)=>(g.text.clone(),"",Some(g.column),None),
                Result_::Git(g)=>(g.text.clone(),g.status.marker(),None,None),
                Result_::Buffer(b)=>(b.text.clone(),"",None,None),
                Result_::Symbol(s)=>(s.name.clone(),"",Some(s.column),Some(s.kind.as_str())),
            };
            json!({"index":index,"path":r.path(),"line":r.line(),"column":column,"text":text,"marker":marker,"symbol":symbol,"score":score})
        }).collect::<Vec<Value>>());
        let target=self.selected_open_target().map(|(path,line)|json!({"path":path.to_string_lossy(),"line":line,"column":self.selected_reference_target().map(|t|t.2).or_else(||self.selected_symbol_target().map(|t|t.1)),"mode":mode_name(self.mode),"query":self.query}));
        let preview_path = self.results.get(self.selected_index)
            .map(|(_, r)| r.path())
            .filter(|p| !p.is_empty())
            .map(ToString::to_string);
        let preview_x = inner_x + left_w + COLUMN_DIVIDER_WIDTH + PREVIEW_PADDING * scale;
        let preview_y = inner_y + PREVIEW_PADDING * scale;
        let preview_w =
            (inner_x + inner_w - preview_x - PREVIEW_PADDING * scale).max(0.0);
        let preview_h = (inner_h - PREVIEW_PADDING * scale * 2.0).max(0.0);
        let mut preview_rows = Vec::new();
        let mut preview_offset = 0.0;
        if preview_active && self.preview_content_path == preview_path {
            let total = self.preview_content_lines.len();
            if total > 0 {
                let target =
                    self.selected_line().unwrap_or(1).max(1).min(total as u32) as usize;
                let line_height = PREVIEW_LINE_HEIGHT * scale;
                let max_lines =
                    ((preview_h - line_height) / line_height).floor().max(1.0) as usize;
                let max_lines = max_lines.min(PREVIEW_MAX_LINES);
                let start = target.saturating_sub(max_lines / 2);
                let end = (start + max_lines).min(total);
                let start = end.saturating_sub(max_lines).max(1);
                if let Some(path) = preview_path.as_deref() {
                    self.push_preview_target(path, start as u32, line_height);
                }
                preview_offset = self.tick_preview();
                let overscan = ((preview_offset.abs() / line_height).ceil() as usize)
                    .saturating_add(1);
                for line in
                    start.saturating_sub(overscan).max(1)..=(end + overscan).min(total)
                {
                    let max_chars = ((preview_w / (PREVIEW_FONT_SIZE * scale * 0.6))
                        .floor() as usize)
                        .saturating_sub(8);
                    let text: String = self.preview_content_lines[line - 1]
                        .chars()
                        .take(max_chars)
                        .collect();
                    let language = crate::syntax::Lang::from_path(
                        preview_path.as_deref().unwrap_or(""),
                    );
                    let tokens: Vec<Value> = crate::syntax::highlight_line(
                        &text, language,
                    )
                    .into_iter()
                    .map(
                        |(token, text)| json!({"token":format!("{token:?}"),"text":text}),
                    )
                    .collect();
                    preview_rows.push(json!({"line":line,"text":text,"tokens":tokens,"y":line_height+(line as i64-start as i64)as f32*line_height+preview_offset,"selected":self.selected_line()==Some(line as u32)}));
                }
            }
        }
        let now = Instant::now();
        let next_tick = self.query_dirty_at.map(|dirty| {
            self.search_debounce_ms()
                .saturating_sub(now.saturating_duration_since(dirty).as_millis())
                + 1
        });
        let pop_active = self.pop_on_open
            && now
                .saturating_duration_since(self.open_pop_started)
                .as_millis()
                < (OPEN_POP_MS as u128);
        let animating = pop_active
            || self.list_scroll_spring.position.abs() > 0.01
            || cursor_offset.abs() > 0.01
            || preview_offset.abs() > 0.01;
        let placeholder = match self.mode {
            FinderMode::Files => "Find file in project...",
            FinderMode::Grep => "Grep across project...",
            FinderMode::GitChanges => "Filter git changes...",
            FinderMode::BufferLines => "Search in buffer...",
            FinderMode::BufferReplace => "Replace: pattern/replacement...",
            FinderMode::References => "Filter references...",
            FinderMode::Symbols => "Go to symbol...",
        };
        let empty = if self.mode == FinderMode::Symbols && self.symbol_rows.is_empty() {
            if self.symbols_loading {
                "Waiting for language server…"
            } else {
                "No symbols in this file"
            }
        } else if self.query.is_empty() {
            match self.mode {
                FinderMode::Files => "No files found",
                FinderMode::Grep => "Start typing to grep…",
                FinderMode::GitChanges => "No git changes",
                FinderMode::BufferLines => "Start typing to search buffer…",
                FinderMode::BufferReplace => {
                    "Type pattern/replacement — Enter replaces all"
                }
                FinderMode::References => "No references",
                FinderMode::Symbols => "No symbols in this file",
            }
        } else if self.grep_query_too_short(&self.effective_search_key()) {
            "Type at least 2 chars to grep…"
        } else {
            "No matches"
        };
        let mut frame = json!({"enabled":self.enabled,"mode":mode_name(self.mode),"query":self.query,"rect":[x,y,w,h],"radius":FINDER_RADIUS*scale,"stroke":stroke,
            "inputRect":[inner_x,inner_y,left_w,input_h],"listRect":[inner_x,results_y,left_w,list_h],"previewRect":[preview_x,preview_y,preview_w,preview_h],
            "rowHeight":row_h,"inputFontSize":INPUT_FONT_SIZE*scale,"fontSize":RESULT_FONT_SIZE*scale,"previewFontSize":PREVIEW_FONT_SIZE*scale,"previewLineHeight":PREVIEW_LINE_HEIGHT*scale,
            "selected":self.selected_index,"selectedY":selected_y,"offset":offset,"top":self.scroll_offset,"visibleRows":visible_rows,"target":target,
            "badge":self.search_mode_label(),"matchTotal":self.buffer_match_total,"placeholder":placeholder,"empty":empty,"preview":preview_active,"previewPath":preview_path,
            "previewRows":preview_rows,"previewUnreadable":self.preview_content_unreadable,"previewLoaded":self.preview_content_path==preview_path,"animating":animating,"nextTickMs":next_tick});
        if let Some(rows) = rows { frame["rows"] = json!(rows); }
        frame
    }

    /// Keep the original painter's measured caret geometry, but separate its
    /// command so retained hosts can blink without rerunning Finder/render.
    pub fn host_render(
        &mut self,
        paint: &mut crate::primitives::paint_record::RecordingPainter,
        size: (f32, f32, f32),
        theme: &crate::primitives::ide_theme::IdeTheme,
        search: &dyn crate::services::SearchService,
        files: &dyn crate::services::FilesService,
    ) -> (Value, Value) {
        let blink_start = self.caret_blink_start;
        self.caret_blink_start = Instant::now();
        // The host ticks before constructing metadata and deciding whether to
        // send rows. Do not let render's legacy tick cross a debounce deadline
        // and mutate results after that decision.
        let query_dirty_at = self.query_dirty_at.take();
        self.render(paint, size, theme, search, files);
        self.query_dirty_at = query_dirty_at;
        self.caret_blink_start = blink_start;
        let mut commands = json!(paint.serializable_commands(false));
        // The input caret is the only element-depth, square-cornered rect of
        // this size. Use the shared constants, not a host-side geometry copy.
        let index = commands.as_array().and_then(|commands| commands.iter().position(|c| {
            c["kind"] == "rect"
                && c["rect"][2] == json!(CARET_WIDTH * self.scale)
                && c["rect"][3] == json!(INPUT_FONT_SIZE * self.scale + 4.0)
                && c["depth"] == json!(DEPTH_ELEMENT)
                && c["order"] == json!(ORDER)
                && c["radii"] == json!([0.0, 0.0, 0.0, 0.0])
        }));
        let caret = index.map(|index| commands.as_array_mut().unwrap().remove(index)).unwrap_or(Value::Null);
        (commands, caret)
    }
}
