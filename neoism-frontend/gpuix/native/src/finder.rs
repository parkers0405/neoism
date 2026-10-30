//! Async daemon capability adapter around the existing Finder engine.
use neoism_ui::{
    panels::finder::{Finder, ReferenceRow, SymbolRow},
    primitives::{ide_theme::IdeTheme, paint_record::RecordingPainter},
    panels::trail_cursor::TrailCursor,
    services::{
        DirEntry, FilesService, IoError, SearchDirectoryHit, SearchFileHit, SearchFileMode, SearchGitHit,
        SearchGrepHit, SearchGrepMode, SearchService,
    },
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Instant,
};

#[derive(Default)]
struct Requests {
    next: u64,
    cache: HashMap<String, Value>,
    pending: HashMap<String, u64>,
    keys: HashMap<u64, String>,
    outgoing: Vec<Value>,
}
#[derive(Default)]
struct RemoteSearch {
    inner: Mutex<Requests>,
}
impl RemoteSearch {
    fn request(&self, tag: &str, mut args: Value) -> Result<Value, IoError> {
        let key = format!("{tag}:{args}");
        let mut state = self.inner.lock().unwrap();
        if let Some(cached) = state.cache.get(&key) {
            return Ok(cached.clone());
        }
        if let Some(id) = state.pending.get(&key) {
            return Err(IoError::Pending(*id));
        }
        state.next += 1;
        let id = state.next;
        args["req_id"] = json!(id);
        state.pending.insert(key.clone(), id);
        state.keys.insert(id, key);
        state.outgoing.push(json!({"id":id,"message":{tag:args}}));
        Err(IoError::Pending(id))
    }
    fn clear_pending(&self) {
        let mut state = self.inner.lock().unwrap();
        state.pending.clear();
        state.keys.clear();
        state.outgoing.clear();
    }
    fn reply(&self, id: u64, payload: &Value) -> bool {
        let mut state = self.inner.lock().unwrap();
        let Some(key) = state.keys.remove(&id) else {
            return false;
        };
        state.pending.remove(&key);
        let Some(body) = payload.as_object().and_then(|o| o.values().next()) else {
            return false;
        };
        if state.cache.len() > 100 {
            state.cache.clear();
        }
        state.cache.insert(key, body.clone());
        true
    }
    fn take(&self) -> Vec<Value> {
        std::mem::take(&mut self.inner.lock().unwrap().outgoing)
    }
    fn count(&self) -> usize {
        self.inner.lock().unwrap().pending.len()
    }
}
fn io(error: serde_json::Error) -> IoError {
    IoError::Other(error.to_string())
}
impl SearchService for RemoteSearch {
    fn collect_files(&self, _cwd: &Path) -> Result<Vec<String>, IoError> {
        serde_json::from_value(
            self.request("CollectFiles", json!({"cwd":""}))?["paths"].clone(),
        )
        .map_err(io)
    }
    fn search_files(
        &self,
        _cwd: &Path,
        query: &str,
        mode: SearchFileMode,
    ) -> Result<Vec<SearchFileHit>, IoError> {
        serde_json::from_value(
            self.request(
                "SearchFiles",
                json!({"cwd":"","query":query,"mode":format!("{mode:?}")}),
            )?["hits"]
                .clone(),
        )
        .map_err(io)
    }
    fn search_directories(
        &self,
        _cwd: &Path,
        query: &str,
    ) -> Result<Vec<SearchDirectoryHit>, IoError> {
        serde_json::from_value(
            self.request("SearchDirectories", json!({"cwd":"","query":query}))?["hits"]
                .clone(),
        )
        .map_err(io)
    }
    fn search_grep(
        &self,
        _cwd: &Path,
        query: &str,
        mode: SearchGrepMode,
    ) -> Result<Vec<SearchGrepHit>, IoError> {
        serde_json::from_value(self.request("SearchGrep",json!({"cwd":"","query":query,"mode":format!("{mode:?}"),"case_sensitive":null,"file_patterns":[]}))?["hits"].clone()).map_err(io)
    }
    fn collect_git_changes(&self, _cwd: &Path) -> Result<Vec<SearchGitHit>, IoError> {
        serde_json::from_value(
            self.request("SearchGitChanges", json!({"cwd":""}))?["hits"].clone(),
        )
        .map_err(io)
    }
    fn git_repo_root(&self, _cwd: &Path) -> Option<PathBuf> {
        None
    }
}
struct HostFinder {
    finder: Finder,
    search: RemoteSearch,
    width: f32,
    height: f32,
    paint: RecordingPainter,
    theme: String,
    cursor: TrailCursor,
    cursor_frame: Instant,
}
/// Preview bytes are installed by the owning daemon's Files capability. The
/// shared renderer must never read a similarly named file on this UI host.
struct RemotePreview;
impl FilesService for RemotePreview {
    fn list_dir(&self,_:&Path)->Result<Vec<DirEntry>,IoError>{Err(IoError::Other("Finder preview is read-only".into()))}
    fn read_file(&self,_:&Path)->Result<Vec<u8>,IoError>{Err(IoError::Pending(0))}
    fn write_file(&self,_:&Path,_:&[u8])->Result<(),IoError>{Err(IoError::Other("Finder preview is read-only".into()))}
    fn stat(&self,_:&Path)->Result<DirEntry,IoError>{Err(IoError::Other("Finder preview has no local filesystem".into()))}
}
impl HostFinder {
    fn frame(&mut self) -> Value {
        self.finder.tick(&self.search);
        let mut frame = self.finder.host_frame(self.width, self.height);
        self.paint.begin_frame();
        self.finder.render(&mut self.paint,(self.width,self.height,1.0),&IdeTheme::by_name(&self.theme),&self.search,&RemotePreview);
        frame["commands"] = json!(self.paint.commands());
        if let Some(rect)=self.finder.selected_cursor_rect(){
            let was_moving=self.cursor.is_animating();let now=Instant::now();
            self.cursor.set_cursor_shape(neoism_terminal_types::ansi::CursorShape::Block);
            self.cursor.set_destination(rect[0],rect[1],rect[2],rect[3]);
            self.cursor.animate(rect[2],rect[3],if was_moving{now.duration_since(self.cursor_frame).as_secs_f32()}else{0.0});self.cursor_frame=now;
            frame["cursorQuad"]=json!(self.cursor.quad_points());
            frame["cursorHold"]=json!(self.cursor.blink_hold_visible(neoism_ui::editor::optimistic::DEFAULT_BLINK_PERIOD_MS));
            frame["animating"]=json!(frame["animating"].as_bool().unwrap_or(false)||self.cursor.is_animating());
        }else{frame["cursorQuad"]=json!([]);}
        frame["requests"] = json!(self.search.take());
        frame["loading"] = json!(self.search.count() > 0);
        frame
    }
}
#[derive(Default)]
struct Store {
    next: u32,
    finders: HashMap<u32, HostFinder>,
}
thread_local! {static STORE:RefCell<Store>=RefCell::new(Store::default());}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn number(v: &Value, key: &str, fallback: f32) -> f32 {
    v[key]
        .as_f64()
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .unwrap_or(fallback)
}
fn strings(v: &Value, key: &str) -> Vec<String> {
    v[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}
fn symbols(v: &Value) -> Vec<SymbolRow> {
    v["symbols"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| SymbolRow {
            kind: text(r, "kind").into(),
            name: text(r, "name").into(),
            line: r["line"].as_u64().unwrap_or(1) as u32,
            column: r["column"].as_u64().unwrap_or(0) as u32,
        })
        .collect()
}
pub fn dispatch(v: &Value) -> Option<Result<Value, String>> {
    let op = text(v, "op");
    if !op.starts_with("finder.") {
        return None;
    }
    Some(STORE.with(|store| {
        let mut store = store.borrow_mut();
        if op == "finder.new" {
            store.next += 1;
            let id = 0x60000000 + store.next;
            let search = RemoteSearch::default();
            let mut finder = Finder::new();
            finder.set_scale(number(v, "scale", 1.0));
            finder.set_top_anchor(number(v, "top", 80.0));
            let cwd = PathBuf::from(text(v, "cwd"));
            match text(v, "mode") {
                "grep" => finder.open_grep(cwd),
                "git" => finder.open_git_changes(&search, cwd),
                "buffer" => finder.open_buffer_lines(strings(v, "lines")),
                "replace" => finder.open_buffer_replace(strings(v, "lines")),
                "references" => finder.open_references(
                    cwd,
                    v["references"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|r| ReferenceRow {
                            path: text(r, "path").into(),
                            line: r["line"].as_u64().unwrap_or(1) as u32,
                            column: r["column"].as_u64().unwrap_or(0) as u32,
                            text: text(r, "text").into(),
                        })
                        .collect(),
                ),
                "symbols" => {
                    finder.open_symbols();
                    finder.set_symbol_rows(symbols(v));
                    finder.set_symbols_loading(v["loading"].as_bool().unwrap_or(false));
                }
                _ => finder.open_files(cwd),
            }
            let mut host = HostFinder {
                finder,
                search,
                width: number(v, "width", 1440.0),
                height: number(v, "height", 900.0),
                paint: RecordingPainter::new(v["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),v["fontWeight"].as_u64().unwrap_or(400) as u16,vec![v["iconFontFamily"].as_str().unwrap_or("Symbols Nerd Font Mono").into()]),
                theme: v["theme"].as_str().unwrap_or("pastel_dark").into(),
                cursor: TrailCursor::new(),
                cursor_frame: Instant::now(),
            };
            let frame = host.frame();
            store.finders.insert(id, host);
            return Ok(json!({"id":id,"frame":frame}));
        }
        let id = v["id"].as_u64().unwrap_or(0) as u32;
        if op == "finder.dispose" {
            store.finders.remove(&id);
            return Ok(json!({}));
        }
        let host = store.finders.get_mut(&id).ok_or("Unknown Finder")?;
        match op {
            "finder.frame" => {}
            "finder.style" => {
                host.theme=v["theme"].as_str().unwrap_or("pastel_dark").into();
                host.paint=RecordingPainter::new(v["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),v["fontWeight"].as_u64().unwrap_or(400) as u16,vec![v["iconFontFamily"].as_str().unwrap_or("Symbols Nerd Font Mono").into()]);
            }
            "finder.query" => {
                host.search.clear_pending();
                host.finder.set_query(text(v, "query").into());
            }
            "finder.cycle" => {
                host.search.clear_pending();
                host.finder.cycle_search_mode();
            }
            "finder.move" => {
                let delta = v["delta"].as_i64().unwrap_or(1);
                let visible = host.finder.host_frame(host.width, host.height)
                    ["visibleRows"]
                    .as_u64()
                    .unwrap_or(8) as usize;
                for _ in 0..delta.unsigned_abs().min(500) {
                    if delta < 0 {
                        host.finder.move_selection_up();
                    } else {
                        host.finder.move_selection_down(visible);
                    }
                }
            }
            "finder.select" => host
                .finder
                .select_index(v["index"].as_u64().unwrap_or(0) as usize),
            "finder.scroll" => host.finder.scroll_pixels(number(v, "delta", 0.0)),
            "finder.touch-scroll" => {
                host.finder.scroll_touch_pixels(number(v, "delta", 0.0));
            }
            "finder.layout" => {
                host.width = number(v, "width", host.width).max(1.0);
                host.height = number(v, "height", host.height).max(1.0);
            }
            "finder.reply" => {
                let request = v["requestId"].as_u64().unwrap_or(0);
                if host.search.reply(request, &v["message"]) {
                    host.finder.handle_service_reply(
                        request,
                        &v["message"],
                        &host.search,
                    );
                }
            }
            "finder.preview" => host.finder.host_set_preview(
                text(v, "path").into(),
                v["text"].as_str().map(ToString::to_string),
            ),
            "finder.symbols" => {
                host.finder.set_symbol_rows(symbols(v));
                host.finder
                    .set_symbols_loading(v["loading"].as_bool().unwrap_or(false));
            }
            _ => return Err(format!("Unknown Finder operation: {op}")),
        }
        Ok(host.frame())
    }))
}
