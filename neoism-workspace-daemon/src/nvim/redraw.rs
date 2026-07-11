use super::*;

// -----------------------------------------------------------------------
// nvim-rs Handler — redraw event sink
// -----------------------------------------------------------------------

/// Resolved highlight attributes (rgb fg/bg + attr bitfield).
#[derive(Clone, Copy, Default)]
pub(crate) struct ResolvedHl {
    fg: Option<u32>,
    bg: Option<u32>,
    sp: Option<u32>,
    attrs: u8,
}

impl From<ResolvedHl> for HighlightAttrs {
    fn from(value: ResolvedHl) -> Self {
        Self {
            fg: value.fg,
            bg: value.bg,
            sp: value.sp,
            bold: value.attrs & 0b0000_0001 != 0,
            italic: value.attrs & 0b0000_0010 != 0,
            underline: value.attrs & 0b0000_0100 != 0,
            undercurl: value.attrs & 0b0000_1000 != 0,
            strikethrough: value.attrs & 0b0001_0000 != 0,
            reverse: value.attrs & 0b0010_0000 != 0,
        }
    }
}

#[derive(Default)]
pub(crate) struct HighlightTable {
    map: HashMap<u64, ResolvedHl>,
}

/// Per-grid pending state accumulated between `grid_line` events and
/// the next `flush`. nvim batches updates this way so the UI only
/// redraws once per coherent change.
#[derive(Default)]
pub(crate) struct GridPending {
    width: u32,
    height: u32,
    cells: Vec<GridCell>,
    cursor: Option<GridPos>,
    mode: Option<String>,
}

/// Last-seen cursor position and mode, kept on the handler so we can
/// re-emit cursor-overlay frames when the mode changes without the
/// cursor cell moving (e.g. typing `i` in normal mode).
#[derive(Clone, Copy, Default)]
pub(crate) struct LastCursor {
    /// 0-based row, 0-based col on grid 1 (the primary editor grid).
    /// `None` until the first `grid_cursor_goto` arrives.
    pos: Option<(u32, u32)>,
    /// Last `mode_change` short name. `""` until set.
    mode: ModeStr,
}

/// Inline fixed-capacity string for the mode name. Keeps `LastCursor`
/// `Copy` (avoids needing a `Mutex<LastCursor>` clone on every read)
/// without dragging in `arrayvec` for one field. The longest mode
/// nvim emits is `"cmdline_normal"` (15 chars) — we round up to 16.
#[derive(Clone, Copy)]
pub(crate) struct ModeStr {
    buf: [u8; 16],
    len: u8,
}

impl Default for ModeStr {
    fn default() -> Self {
        Self {
            buf: [0u8; 16],
            len: 0,
        }
    }
}

impl ModeStr {
    fn set(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let n = bytes.len().min(self.buf.len());
        self.buf[..n].copy_from_slice(&bytes[..n]);
        for slot in &mut self.buf[n..] {
            *slot = 0;
        }
        self.len = n as u8;
    }
    fn as_str(&self) -> &str {
        // SAFETY: only ever populated from a `&str` in `set`, so the
        // prefix is valid utf-8.
        std::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("")
    }
}

#[derive(Clone)]
pub(crate) struct RedrawHandler {
    pub(crate) redraw_tx: mpsc::UnboundedSender<EditorServerMessage>,
    /// Cursor-overlay push channel — separate from the redraw stream
    /// so the daemon's `select!` arms in `server.rs` can route each
    /// family through its own envelope tag.
    pub(crate) cursor_overlay_tx: mpsc::UnboundedSender<CursorOverlayServerMessage>,
    /// Wave 6C nvim→CRDT bridge: incremental buffer-line changes
    /// reported by the lua `on_lines` attach. NOT gated on
    /// `redraw_enabled` — document sync is independent of grid paint.
    pub(crate) buffer_lines_tx: mpsc::UnboundedSender<NvimBufferEvent>,
    pub(crate) hl_table: Arc<Mutex<HighlightTable>>,
    pub(crate) default_fg: Arc<Mutex<u32>>,
    pub(crate) default_bg: Arc<Mutex<u32>>,
    pub(crate) grid_sizes: Arc<Mutex<HashMap<u32, (u32, u32)>>>,
    /// Last cursor cell + mode, used to re-emit a `TrailCursor` /
    /// `CursorlineOverlay` pair when only the mode changes (i.e. the
    /// cursor cell didn't move but the shape should flip from block
    /// to beam, etc.).
    pub(crate) last_cursor: Arc<Mutex<LastCursor>>,
    /// Latest client-addressed web pane route id. Until the daemon
    /// owns independent nvim grids, redraws are stamped with this
    /// active surface so clients can route snapshots per pane.
    pub(crate) active_surface_id: Arc<Mutex<Option<String>>>,
    pub(crate) redraw_enabled: Arc<Mutex<bool>>,
    pub(crate) redraw_batch: Arc<Mutex<Option<Vec<EditorServerMessage>>>>,
    /// Latest gutter width (cells) reported by the `neoism_textoff`
    /// autocmd — stamped into outgoing `WinViewport`s so clients can
    /// place buffer-column carets in the text area.
    pub(crate) textoff: Arc<Mutex<u64>>,
}

#[async_trait]
impl Handler for RedrawHandler {
    type Writer = NeovimWriter;

    async fn handle_notify(
        &self,
        name: String,
        args: Vec<Value>,
        _neovim: Neovim<Self::Writer>,
    ) {
        // Out-of-band notifications fired by our injected lua
        // (`apply_ide_defaults`). The yank-flash autocmd reports the
        // affected screen-row range so the chrome can paint a fading
        // highlight without re-deriving viewport offsets here.
        if name == "rio_yank_flash" {
            // args = [row_top, row_bot, col_left?, col_right?],
            // 0-based screen rows/cols already clamped by lua.
            let row_top = args.first().and_then(value_as_u64).unwrap_or(0) as u32;
            let row_bot =
                args.get(1).and_then(value_as_u64).unwrap_or(row_top as u64) as u32;
            let col_left = args.get(2).and_then(value_as_u64).map(|v| v as u32);
            let col_right = args.get(3).and_then(value_as_u64).map(|v| v as u32);
            let surface_id = self.active_surface_id.lock().await.clone();
            self.emit_redraw(EditorServerMessage::YankFlash {
                surface_id,
                row_top,
                row_bot,
                col_left,
                col_right,
            })
            .await;
            let _ = self
                .cursor_overlay_tx
                .send(CursorOverlayServerMessage::YankFlash {
                    regions: vec![YankFlashRegion {
                        row_top,
                        row_bot,
                        col_left,
                        col_right,
                    }],
                });
            return;
        }
        if name == "neoism_crdt_lines" {
            // args = [path, firstline, lastline, new_line_count, new_text]
            // fired by the Wave 6C on_lines attach (see
            // `attach_buffer_change_events`). Echo-suppressed at the lua
            // layer; everything arriving here is a genuine nvim-side edit.
            let Some(path) = args.first().and_then(|v| v.as_str()) else {
                return;
            };
            if path.is_empty() {
                return;
            }
            let firstline = args.get(1).and_then(value_as_u64).unwrap_or(0);
            let lastline = args.get(2).and_then(value_as_u64).unwrap_or(firstline);
            let new_line_count = args.get(3).and_then(value_as_u64).unwrap_or(0);
            let new_text = args
                .get(4)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            tracing::info!(
                target: "neoism::crdt_fold",
                path,
                firstline,
                lastline,
                new_line_count,
                "[crdt-fold] on_lines notify arrived from nvim"
            );
            let _ = self.buffer_lines_tx.send(NvimBufferEvent::Lines(
                NvimBufferLinesChange {
                    path: PathBuf::from(path),
                    firstline,
                    lastline,
                    new_line_count,
                    new_text,
                },
            ));
            return;
        }
        if name == "neoism_textoff" {
            if let Some(value) = args.first().and_then(value_as_u64) {
                *self.textoff.lock().await = value;
            }
            return;
        }
        if name == "neoism_crdt_write" {
            // args = [path] — fired by the BufWriteCmd interception in
            // `attach_buffer_change_events`. nvim no longer writes the
            // file itself; the daemon flushes the authoritative doc.
            let Some(path) = args.first().and_then(|v| v.as_str()) else {
                return;
            };
            if path.is_empty() {
                return;
            }
            let _ = self.buffer_lines_tx.send(NvimBufferEvent::WriteRequested {
                path: PathBuf::from(path),
            });
            return;
        }
        if name == "rio_buf_enter" {
            let Some(path) = args.first().and_then(|v| v.as_str()) else {
                return;
            };
            let line_count = args.get(1).and_then(value_as_u64).unwrap_or(0);
            let surface_id = self.active_surface_id.lock().await.clone();
            self.emit_redraw(EditorServerMessage::BufferOpened {
                surface_id,
                path: PathBuf::from(path),
                line_count,
            })
            .await;
            return;
        }
        if name == "rio_notify" {
            let Some(message) = args.first().and_then(|v| v.as_str()) else {
                return;
            };
            let level = args
                .get(1)
                .and_then(|v| v.as_str())
                .unwrap_or("info")
                .to_string();
            let surface_id = self.active_surface_id.lock().await.clone();
            self.emit_redraw(EditorServerMessage::Notification {
                surface_id,
                message: message.to_string(),
                level,
            })
            .await;
            return;
        }
        if name == "rio_modal" || name == "rio_modal_actions" {
            let title = args.first().and_then(|v| v.as_str()).unwrap_or("Nvim");
            let body = args.get(1).and_then(|v| v.as_str()).unwrap_or("");
            let level = args
                .get(2)
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .to_string();
            let message = if body.is_empty() {
                title.to_string()
            } else {
                format!("{title}: {body}")
            };
            tracing::warn!(
                target: "neoism::nvim_notify",
                notify_name = %name,
                %message,
                %level,
                "nvim modal notification"
            );
            let surface_id = self.active_surface_id.lock().await.clone();
            self.emit_redraw(EditorServerMessage::Notification {
                surface_id,
                message,
                level,
            })
            .await;
            return;
        }
        if name == "rio_clipboard_store" {
            if let Some(message) = args.get(2).and_then(|v| v.as_str()) {
                let level = args
                    .get(3)
                    .and_then(|v| v.as_str())
                    .unwrap_or("info")
                    .to_string();
                let surface_id = self.active_surface_id.lock().await.clone();
                self.emit_redraw(EditorServerMessage::Notification {
                    surface_id,
                    message: message.to_string(),
                    level,
                })
                .await;
            }
            return;
        }
        if name == "rio_buf_modified" {
            let Some(path) = args.first().and_then(|v| v.as_str()) else {
                return;
            };
            let modified = args
                .get(1)
                .and_then(|v| match v {
                    Value::Boolean(value) => Some(*value),
                    _ => None,
                })
                .unwrap_or(false);
            let surface_id = self.active_surface_id.lock().await.clone();
            self.emit_redraw(EditorServerMessage::BufferModified {
                surface_id,
                path: PathBuf::from(path),
                modified,
            })
            .await;
            return;
        }
        if name == "rio_lsp_status" {
            // Legacy nvim/Lua pill source. The Rust LSP engine now owns the
            // status-bar pill (rust_lsp::poll → EditorServerMessage::
            // LspSnapshot); ignore the Lua notification so the two sources
            // don't fight over `lsp_snapshot`.
            return;
        }
        if name == "rio_lsp_log" {
            if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                let event = args.first().and_then(|v| v.as_str()).unwrap_or("unknown");
                let fields_json = args.get(1).and_then(|v| v.as_str()).unwrap_or("{}");
                let surface_id = self.active_surface_id.lock().await.clone();
                tracing::info!(
                    target: "neoism::lsp",
                    surface_id = ?surface_id,
                    event = %event,
                    fields = %fields_json,
                    "daemon nvim lsp log"
                );
            }
            return;
        }
        if name == "rio_lsp_snapshot" {
            // Legacy nvim/Lua snapshot, superseded by the Rust LSP engine
            // (rust_lsp::poll builds the pill's server list from the real
            // per-workspace runtime). Ignored to keep the pill single-
            // sourced and avoid flicker between the two producers.
            return;
        }
        if name == "rio_lsp_message" {
            let Some(server) = args.first().and_then(value_as_nonempty_string) else {
                return;
            };
            let Some(text) = args.get(1).and_then(value_as_nonempty_string) else {
                return;
            };
            let surface_id = self.active_surface_id.lock().await.clone();
            self.emit_redraw(EditorServerMessage::LspMessage {
                surface_id,
                server,
                text,
                level: args
                    .get(2)
                    .and_then(value_as_nonempty_string)
                    .unwrap_or_else(|| "info".to_string()),
            })
            .await;
            return;
        }
        if name == "rio_diagnostics" {
            // Legacy nvim/Lua diagnostics source. The Rust LSP engine now owns
            // inline diagnostics too (rust_lsp::poll builds
            // `EditorServerMessage::Diagnostics` from the engine's own
            // publishDiagnostics); ignore the nvim notification so the two
            // don't fight over `editor_diagnostics`.
            return;
        }
        if name != "redraw" {
            // Unknown out-of-band notification — drop quietly so a
            // future lua shim can add more notify names without
            // landing in the redraw decoder.
            return;
        }
        *self.redraw_batch.lock().await = Some(Vec::new());
        // `redraw` args is a flat list of `[event_name, ...calls]`
        // tuples. Each call is itself a list of args. We accumulate
        // pending state per-grid and flush at the end of the batch.
        let mut pending: HashMap<u32, GridPending> = HashMap::new();
        let mut last_hl: u64 = 0;

        for entry in args {
            let Value::Array(items) = entry else { continue };
            let mut iter = items.into_iter();
            let Some(Value::String(event)) = iter.next() else {
                continue;
            };
            let event_name = event.as_str().unwrap_or("").to_string();
            // Remaining items are the per-call arg tuples.
            for call in iter {
                let Value::Array(call_args) = call else {
                    continue;
                };
                self.handle_event(&event_name, call_args, &mut pending, &mut last_hl)
                    .await;
            }
        }

        // Flush every grid we touched. Explicit `flush` events drain
        // this earlier; this catches batches that omit one.
        self.flush_pending(&mut pending).await;
        let messages = self.redraw_batch.lock().await.take().unwrap_or_default();
        match messages.len() {
            0 => {}
            1 => {
                let mut iter = messages.into_iter();
                if let Some(message) = iter.next() {
                    let _ = self.redraw_tx.send(message);
                }
            }
            _ => {
                let surface_id = self.current_surface_id().await;
                let _ = self.redraw_tx.send(EditorServerMessage::Batch {
                    surface_id,
                    messages,
                });
            }
        }
    }

    async fn handle_request(
        &self,
        _name: String,
        _args: Vec<Value>,
        _neovim: Neovim<Self::Writer>,
    ) -> Result<Value, Value> {
        // We don't expose any client-side request handlers yet — every
        // synchronous round-trip nvim might want (clipboard, ui_*)
        // we'll grow as needed.
        Err(Value::from("no request handlers implemented"))
    }
}

pub(crate) fn value_as_nonempty_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string).filter(|s| !s.is_empty())
}

impl RedrawHandler {
    async fn current_surface_id(&self) -> Option<String> {
        self.active_surface_id.lock().await.clone()
    }

    async fn emit_redraw(&self, message: EditorServerMessage) {
        if !*self.redraw_enabled.lock().await {
            return;
        }
        let mut batch = self.redraw_batch.lock().await;
        if let Some(messages) = batch.as_mut() {
            messages.push(message);
        } else {
            let _ = self.redraw_tx.send(message);
        }
    }

    /// Push the paired `TrailCursor` + `CursorlineOverlay` frames for
    /// a primary-grid cursor cell. The web dispatcher multiplies
    /// `(col, row)` by its cell metrics to land on the same pixel the
    /// desktop renderer would compute in its `set_destination` call.
    fn emit_cursor_overlay_for_pos(&self, row: u32, col: u32, shape: ProtoCursorShape) {
        let _ = self
            .cursor_overlay_tx
            .send(CursorOverlayServerMessage::TrailCursor {
                col,
                row,
                shape: Some(shape),
                no_jump: false,
                reset: false,
                snap: false,
            });
        // Cursorline always anchors to the row the cursor sits on.
        // `rich_text_id = 0` because grid 1 is the primary editor
        // pane; the web bridge maps that to its single editor pane
        // today. When multi-pane lands, the daemon will need a real
        // pane-id table — for now the web client only renders one.
        let _ =
            self.cursor_overlay_tx
                .send(CursorOverlayServerMessage::CursorlineOverlay {
                    rich_text_id: 0,
                    target_row: row,
                    snap: false,
                    forget: false,
                });
    }

    async fn flush_pending(&self, pending: &mut HashMap<u32, GridPending>) {
        let drained = std::mem::take(pending);
        if drained.is_empty() {
            return;
        }

        let grid_sizes = self.grid_sizes.lock().await;
        for (grid_id, p) in drained {
            let (width, height) = if p.width > 0 && p.height > 0 {
                (p.width, p.height)
            } else if let Some(size) = grid_sizes.get(&grid_id).copied() {
                size
            } else {
                // Unknown grid with no resize seen yet: GUESSING the
                // default 80x24 here stamped wrong dims onto the wire
                // — clients derive cell size from grid dims, so one
                // guessed frame = a one-frame zoom snap. nvim always
                // sends grid_resize before content for a new grid, so
                // skipping loses nothing.
                tracing::debug!(grid_id, "skipping redraw for unsized grid");
                continue;
            };
            self.emit_redraw(EditorServerMessage::GridUpdate {
                surface_id: self.current_surface_id().await,
                grid_id,
                width,
                height,
                cells: p.cells,
                cursor: p.cursor,
                mode: p.mode,
            })
            .await;
        }
    }

    pub(crate) async fn handle_event(
        &self,
        name: &str,
        args: Vec<Value>,
        pending: &mut HashMap<u32, GridPending>,
        last_hl: &mut u64,
    ) {
        match name {
            "grid_resize" => {
                // [grid, width, height]
                let grid = as_u32(args.first()).unwrap_or(1);
                let width = as_u32(args.get(1)).unwrap_or(0);
                let height = as_u32(args.get(2)).unwrap_or(0);
                let entry = pending.entry(grid).or_default();
                entry.width = width;
                entry.height = height;
                self.grid_sizes.lock().await.insert(grid, (width, height));
                self.emit_redraw(EditorServerMessage::GridResize {
                    surface_id: self.current_surface_id().await,
                    grid_id: grid,
                    width,
                    height,
                })
                .await;
            }
            "grid_clear" => {
                self.flush_pending(pending).await;
                let grid = as_u32(args.first()).unwrap_or(1);
                self.emit_redraw(EditorServerMessage::GridClear {
                    surface_id: self.current_surface_id().await,
                    grid_id: grid,
                })
                .await;
            }
            "grid_scroll" => {
                // [grid, top, bot, left, right, rows, cols]
                //
                // Preserve nvim's event order: any `grid_line` updates
                // before the scroll must land before the client copies
                // screen cells, and the scrolled-in rows arrive as later
                // `grid_line` updates.
                self.flush_pending(pending).await;

                let grid = as_u32(args.first()).unwrap_or(1);
                let top = as_u32(args.get(1)).unwrap_or(0);
                let bot = as_u32(args.get(2)).unwrap_or(0);
                let left = as_u32(args.get(3)).unwrap_or(0);
                let right = as_u32(args.get(4)).unwrap_or(0);
                let rows = args.get(5).and_then(value_as_i64).unwrap_or(0) as i32;
                let cols = args.get(6).and_then(value_as_i64).unwrap_or(0) as i32;

                self.emit_redraw(EditorServerMessage::GridScroll {
                    surface_id: self.current_surface_id().await,
                    grid_id: grid,
                    top,
                    bot,
                    left,
                    right,
                    rows,
                    cols,
                })
                .await;
            }
            "win_viewport" => {
                // [grid, win, topline, botline, curline, curcol, line_count, scroll_delta]
                //
                // nvim sends this after grid damage in the same redraw
                // batch. Flush pending grid updates first so the client
                // applies the new viewport cells before seeding the
                // Neovide-style scroll animation from `scroll_delta`.
                self.flush_pending(pending).await;

                if args.len() < 7 {
                    return;
                }
                let grid = as_u32(args.first()).unwrap_or(1);
                let topline = args.get(2).and_then(value_as_u64).unwrap_or(0);
                let botline = args.get(3).and_then(value_as_u64).unwrap_or(0);
                let curline = args.get(4).and_then(value_as_u64).unwrap_or(0);
                let curcol = args.get(5).and_then(value_as_u64).unwrap_or(0);
                let line_count = args.get(6).and_then(value_as_u64).unwrap_or(0);
                let scroll_delta = args.get(7).and_then(value_as_f64).unwrap_or(0.0);

                let textoff = *self.textoff.lock().await;
                self.emit_redraw(EditorServerMessage::WinViewport {
                    surface_id: self.current_surface_id().await,
                    grid_id: grid,
                    topline,
                    botline,
                    line_count,
                    scroll_delta,
                    curline,
                    curcol,
                    textoff,
                })
                .await;
            }
            "grid_line" => {
                // [grid, row, col_start, cells]
                let grid = as_u32(args.first()).unwrap_or(1);
                let row = as_u32(args.get(1)).unwrap_or(0);
                let col_start = as_u32(args.get(2)).unwrap_or(0);
                let Some(Value::Array(cells)) = args.get(3).cloned() else {
                    return;
                };
                let table = self.hl_table.lock().await;
                let default_fg = *self.default_fg.lock().await;
                let default_bg = *self.default_bg.lock().await;
                let entry = pending.entry(grid).or_default();
                let mut col = col_start;
                for cell in cells {
                    let Value::Array(parts) = cell else { continue };
                    let text = parts
                        .first()
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(id) = parts.get(1).and_then(value_as_u64) {
                        *last_hl = id;
                    }
                    let repeat = parts.get(2).and_then(value_as_u64).unwrap_or(1) as u32;
                    let resolved = table.map.get(last_hl).copied().unwrap_or_default();
                    let fg = resolved.fg.unwrap_or(default_fg);
                    let bg = resolved.bg.unwrap_or(default_bg);
                    for _ in 0..repeat {
                        entry.cells.push(GridCell {
                            row,
                            col,
                            ch: text.clone(),
                            fg,
                            bg,
                            attrs: resolved.attrs,
                        });
                        col += 1;
                    }
                }
            }
            "grid_cursor_goto" => {
                let grid = as_u32(args.first()).unwrap_or(1);
                let row = as_u32(args.get(1)).unwrap_or(0);
                let col = as_u32(args.get(2)).unwrap_or(0);
                pending.entry(grid).or_default().cursor = Some(GridPos { row, col });
                self.flush_pending(pending).await;
                // Mirror to the cursor-overlay channel so the web
                // chrome's trail-cursor + cursorline-overlay animate
                // to the new cell. We only do this for grid 1 (the
                // primary editor grid) — popup / cmdline grids have
                // their own cursor that the chrome doesn't paint.
                if grid == 1 {
                    let mut last = self.last_cursor.lock().await;
                    last.pos = Some((row, col));
                    let shape = ProtoCursorShape::from_mode(last.mode.as_str());
                    drop(last);
                    self.emit_cursor_overlay_for_pos(row, col, shape);
                }
            }
            "default_colors_set" => {
                // [rgb_fg, rgb_bg, rgb_sp, cterm_fg, cterm_bg]
                let rgb_fg = as_u32(args.first()).unwrap_or(0x00FF_FFFF);
                let rgb_bg = as_u32(args.get(1)).unwrap_or(0x0000_0000);
                let rgb_sp = as_u32(args.get(2)).unwrap_or(0x00FF_FFFF);
                *self.default_fg.lock().await = rgb_fg;
                *self.default_bg.lock().await = rgb_bg;
                self.emit_redraw(EditorServerMessage::DefaultColors {
                    surface_id: self.current_surface_id().await,
                    rgb_fg,
                    rgb_bg,
                    rgb_sp,
                })
                .await;
            }
            "hl_attr_define" => {
                // [id, rgb_attrs, cterm_attrs, info]
                let Some(id) = args.first().and_then(value_as_u64) else {
                    return;
                };
                let Some(Value::Map(attrs)) = args.get(1).cloned() else {
                    return;
                };
                let mut resolved = ResolvedHl::default();
                for (k, v) in attrs {
                    let Value::String(key) = k else { continue };
                    let key_str = key.as_str().unwrap_or("");
                    match key_str {
                        "foreground" => resolved.fg = value_as_u64(&v).map(|n| n as u32),
                        "background" => resolved.bg = value_as_u64(&v).map(|n| n as u32),
                        "special" => resolved.sp = value_as_u64(&v).map(|n| n as u32),
                        "bold" if v.as_bool().unwrap_or(false) => {
                            resolved.attrs |= 0b0000_0001
                        }
                        "italic" if v.as_bool().unwrap_or(false) => {
                            resolved.attrs |= 0b0000_0010
                        }
                        "underline" if v.as_bool().unwrap_or(false) => {
                            resolved.attrs |= 0b0000_0100
                        }
                        "undercurl" if v.as_bool().unwrap_or(false) => {
                            resolved.attrs |= 0b0000_1000
                        }
                        "strikethrough" if v.as_bool().unwrap_or(false) => {
                            resolved.attrs |= 0b0001_0000
                        }
                        "reverse" if v.as_bool().unwrap_or(false) => {
                            resolved.attrs |= 0b0010_0000
                        }
                        _ => {}
                    }
                }
                self.hl_table.lock().await.map.insert(id, resolved);
                self.emit_redraw(EditorServerMessage::HighlightDefined {
                    surface_id: self.current_surface_id().await,
                    hl_id: id,
                    attrs: resolved.into(),
                })
                .await;
            }
            "mode_change" => {
                self.flush_pending(pending).await;
                // [mode, mode_idx]
                let mode = args
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mode_idx = as_u32(args.get(1)).unwrap_or(0);
                // Apply to grid 1 (the primary), and also emit a
                // standalone notification for chrome listeners.
                pending.entry(1).or_default().mode = Some(mode.clone());
                // Track the new mode and, if the cursor is known,
                // re-emit a `TrailCursor` so the shape flips (e.g.
                // block → beam on entering insert mode) even if the
                // cell didn't move.
                let mut last = self.last_cursor.lock().await;
                last.mode.set(&mode);
                let pos = last.pos;
                let shape = ProtoCursorShape::from_mode(&mode);
                drop(last);
                if let Some((row, col)) = pos {
                    self.emit_cursor_overlay_for_pos(row, col, shape);
                }
                self.emit_redraw(EditorServerMessage::ModeChange {
                    surface_id: self.current_surface_id().await,
                    mode,
                    mode_idx,
                })
                .await;
            }
            "msg_show" => {
                // [kind, content, replace_last, history]
                //
                // With `ext_messages=true`, `:lua print("hi")`
                // arrives here as kind `lua_print` and content
                // chunks shaped like `[[hl_id, "hi"]]`.
                self.flush_pending(pending).await;
                let kind = args
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let content = decode_msg_show_content(args.get(1));
                let replace_last = args.get(2).and_then(|v| v.as_bool()).unwrap_or(false);
                self.emit_redraw(EditorServerMessage::Message {
                    surface_id: self.current_surface_id().await,
                    kind,
                    content,
                    replace_last,
                })
                .await;
            }
            "popupmenu_show" => {
                // [items, selected, row, col, grid]
                let Some(Value::Array(items)) = args.first().cloned() else {
                    return;
                };
                let selected = args
                    .get(1)
                    .and_then(value_as_i64)
                    .filter(|n| *n >= 0)
                    .map(|n| n as u32);
                let row = as_u32(args.get(2)).unwrap_or(0);
                let col = as_u32(args.get(3)).unwrap_or(0);
                let grid_id = as_u32(args.get(4)).unwrap_or(1);
                let mapped: Vec<neoism_protocol::editor::PopupMenuItem> = items
                    .into_iter()
                    .filter_map(|it| {
                        let Value::Array(parts) = it else { return None };
                        let word = parts
                            .first()
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let kind = parts
                            .get(1)
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let menu = parts
                            .get(2)
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let info = parts
                            .get(3)
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        Some(neoism_protocol::editor::PopupMenuItem {
                            word,
                            kind,
                            menu,
                            info,
                        })
                    })
                    .collect();
                let surface_id = self.current_surface_id().await;
                if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                    tracing::info!(
                        target: "neoism::lsp",
                        surface_id = ?surface_id,
                        items = mapped.len(),
                        selected = ?selected,
                        row,
                        col,
                        grid_id,
                        "daemon popupmenu_show"
                    );
                }
                self.emit_redraw(EditorServerMessage::PopupMenu {
                    surface_id,
                    items: mapped,
                    selected,
                    anchor: GridPos { row, col },
                    grid_id,
                })
                .await;
            }
            "popupmenu_select" => {
                let selected = args
                    .first()
                    .and_then(value_as_i64)
                    .filter(|n| *n >= 0)
                    .map(|n| n as u32);
                self.emit_redraw(EditorServerMessage::PopupMenuSelect {
                    surface_id: self.current_surface_id().await,
                    selected,
                })
                .await;
            }
            "popupmenu_hide" => {
                let surface_id = self.current_surface_id().await;
                if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                    tracing::info!(
                        target: "neoism::lsp",
                        surface_id = ?surface_id,
                        "daemon popupmenu_hide"
                    );
                }
                self.emit_redraw(EditorServerMessage::PopupHide { surface_id })
                    .await;
            }
            "mouse_on" | "mouse_off" => {
                // nvim toggles its mouse grab when entering / leaving
                // buffers that capture the pointer (e.g. `:terminal`,
                // `:Lazy`, popups with `mouse=a`). Forward the toggle
                // as a visibility-only `CustomCursor` push so the web
                // chrome's custom-cursor sprite paints when nvim is
                // *not* eating the pointer and hides when it is. We
                // omit `x` / `y` so the bridge preserves the last
                // cached pointer position — visibility-only frames
                // round-trip via `Option<f32>` on the wire.
                let visible = name == "mouse_off";
                self.emit_redraw(EditorServerMessage::MouseMode {
                    surface_id: self.current_surface_id().await,
                    enabled: name == "mouse_on",
                })
                .await;
                let _ = self.cursor_overlay_tx.send(
                    CursorOverlayServerMessage::CustomCursor {
                        x: None,
                        y: None,
                        visible,
                    },
                );
            }
            "flush" => {
                self.flush_pending(pending).await;
            }
            _ => {
                // Unknown / unhandled events (e.g. `option_set`,
                // `tabline_update`, `cmdline_show`) are silently
                // dropped. Future waves can grow this table.
            }
        }
    }
}

pub(crate) fn decode_msg_show_content(value: Option<&Value>) -> String {
    let Some(Value::Array(chunks)) = value else {
        return String::new();
    };

    let mut out = String::new();
    for chunk in chunks {
        let Value::Array(parts) = chunk else { continue };
        if let Some(text) = parts.get(1).and_then(|v| v.as_str()) {
            out.push_str(text);
        }
    }
    out
}

pub(crate) fn as_u32(v: Option<&Value>) -> Option<u32> {
    v.and_then(value_as_u64).map(|n| n as u32)
}

pub(crate) fn value_as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Integer(i) => i.as_u64().or_else(|| i.as_i64().map(|n| n as u64)),
        _ => None,
    }
}

pub(crate) fn value_as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => i.as_i64(),
        _ => None,
    }
}

pub(crate) fn value_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::F64(n) => Some(*n),
        Value::F32(n) => Some(*n as f64),
        Value::Integer(i) => i.as_i64().map(|n| n as f64),
        _ => None,
    }
}
