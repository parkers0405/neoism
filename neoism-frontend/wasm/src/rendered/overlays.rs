use super::*;
use neoism_protocol::workspace::WorkspaceServerMessage;

#[wasm_bindgen]
impl ChromeBridge {
    // ----- breadcrumbs ------------------------------------------------
    //
    // Wire-shape: `[{ "label": String, "path": Option<String> }, ...]`.
    // The shared `Breadcrumbs::set_segments` only consumes the
    // label list; `path` is accepted (and ignored) so the same
    // envelope can carry click-to-jump paths in a follow-up wave
    // without breaking the contract.

    /// Replace the breadcrumb segment list. Empty array hides the
    /// strip (matches `Breadcrumbs::set_segments`).
    pub fn set_breadcrumbs(&mut self, crumbs_json: &str) -> Result<(), JsValue> {
        #[derive(serde::Deserialize)]
        struct JsCrumb {
            label: String,
            #[serde(default)]
            #[allow(dead_code)]
            path: Option<String>,
        }
        let crumbs: Vec<JsCrumb> = serde_json::from_str(crumbs_json)
            .map_err(|e| JsValue::from_str(&format!("breadcrumbs parse: {e}")))?;
        let segments: Vec<String> = crumbs.into_iter().map(|c| c.label).collect();
        self.chrome.breadcrumbs.set_segments(segments);
        self.relayout_chrome();
        Ok(())
    }

    // ----- completion menu --------------------------------------------
    //
    // Two-piece push: the popup snapshot (`Option`, `None`/`null`
    // hides the menu) plus an editor anchor. The shared panel now
    // stores both via `set_popup` / `set_anchor`; the
    // chrome_shim_more shim reads them so the menu paints whenever
    // a popup is present.

    /// Push the latest popup + anchor. `popup_json` may be `None` to
    /// clear the menu while keeping the last anchor (so the next
    /// `Some(...)` snaps in without a layout glitch).
    pub fn set_completion_menu(
        &mut self,
        popup_json: Option<String>,
        anchor_json: &str,
    ) -> Result<(), JsValue> {
        #[derive(serde::Deserialize, Default)]
        struct JsPopupItem {
            #[serde(default)]
            word: String,
            #[serde(default)]
            kind: String,
            #[serde(default)]
            menu: String,
            #[serde(default)]
            info: String,
        }
        #[derive(serde::Deserialize, Default)]
        struct JsPopup {
            #[serde(default)]
            items: Vec<JsPopupItem>,
            #[serde(default)]
            selected: Option<usize>,
            #[serde(default)]
            anchor_row: u32,
            #[serde(default)]
            anchor_col: u32,
            #[serde(default)]
            grid: u64,
            #[serde(default)]
            max_word_chars: usize,
        }
        #[derive(serde::Deserialize)]
        struct JsAnchor {
            cell_w: f32,
            cell_h: f32,
            panel_left_phys: f32,
            panel_top_phys: f32,
            panel_lines: u32,
            #[serde(default = "default_editor_focused")]
            editor_focused: bool,
        }
        fn default_editor_focused() -> bool {
            true
        }

        let anchor: JsAnchor = serde_json::from_str(anchor_json)
            .map_err(|e| JsValue::from_str(&format!("anchor parse: {e}")))?;
        self.chrome.completion_menu.set_anchor(
            neoism_ui::panels::completion_menu::EditorAnchor {
                cell_w: anchor.cell_w,
                cell_h: anchor.cell_h,
                panel_left_phys: anchor.panel_left_phys,
                panel_top_phys: anchor.panel_top_phys,
                panel_lines: anchor.panel_lines,
                editor_focused: anchor.editor_focused,
            },
        );

        let popup = match popup_json {
            Some(json) => {
                let parsed: JsPopup = serde_json::from_str(&json)
                    .map_err(|e| JsValue::from_str(&format!("popup parse: {e}")))?;
                let items = parsed
                    .items
                    .into_iter()
                    .map(|it| neoism_ui::editor_snapshot::PopupMenuItem {
                        word: it.word,
                        kind: it.kind,
                        menu: it.menu,
                        info: it.info,
                    })
                    .collect();
                Some(neoism_ui::editor_snapshot::PopupMenu {
                    items,
                    selected: parsed.selected,
                    anchor_row: parsed.anchor_row,
                    anchor_col: parsed.anchor_col,
                    grid: parsed.grid,
                    max_word_chars: parsed.max_word_chars,
                })
            }
            None => None,
        };
        self.chrome.completion_menu.set_popup(popup);
        Ok(())
    }

    /// Hide the completion menu and forget the last anchor.
    pub fn dismiss_completion_menu(&mut self) {
        self.chrome.completion_menu.dismiss();
    }

    // ----- minimap ----------------------------------------------------
    //
    // `Minimap::apply_update(route_id, MinimapData)` requires the
    // panel be `enabled`; we flip that on the first push so the
    // host doesn't need a separate enable call. `clear_minimap`
    // drops the cached snapshot for one route via `clear_route`.

    /// Push one route's worth of minimap data. JSON shape mirrors
    /// `neoism_ui::editor_snapshot::MinimapData` (with `path` as a
    /// plain string).
    pub fn set_minimap(
        &mut self,
        route_id: u32,
        update_json: &str,
    ) -> Result<(), JsValue> {
        #[derive(serde::Deserialize)]
        struct JsGitChange {
            line: u64,
            kind: String,
        }
        #[derive(serde::Deserialize)]
        struct JsMinimapData {
            #[serde(default)]
            path: Option<String>,
            #[serde(default)]
            changedtick: u64,
            #[serde(default)]
            total_lines: u64,
            #[serde(default)]
            top_line: u64,
            #[serde(default)]
            bottom_line: u64,
            #[serde(default)]
            cursor_line: u64,
            #[serde(default = "one_u64")]
            sample_stride: u64,
            #[serde(default)]
            lines: Option<Vec<String>>,
            #[serde(default)]
            git_changes: Vec<JsGitChange>,
        }
        fn one_u64() -> u64 {
            1
        }

        let parsed: JsMinimapData = serde_json::from_str(update_json)
            .map_err(|e| JsValue::from_str(&format!("minimap parse: {e}")))?;

        let data = neoism_ui::editor_snapshot::MinimapData {
            path: parsed.path.map(std::path::PathBuf::from),
            changedtick: parsed.changedtick,
            total_lines: parsed.total_lines,
            top_line: parsed.top_line,
            bottom_line: parsed.bottom_line,
            cursor_line: parsed.cursor_line,
            sample_stride: parsed.sample_stride,
            lines: parsed.lines,
            git_changes: parsed
                .git_changes
                .into_iter()
                .map(|g| neoism_ui::editor_snapshot::MinimapGitChange {
                    line: g.line,
                    kind: g.kind,
                })
                .collect(),
        };

        // Flip enabled on first push so `apply_update` actually
        // takes effect — host shouldn't need a separate toggle.
        if !self.chrome.minimap.is_enabled() {
            self.chrome.minimap.set_enabled(true);
        }
        self.chrome.minimap.apply_update(route_id as usize, data);
        Ok(())
    }

    /// Drop the cached minimap snapshot for one route.
    pub fn clear_minimap(&mut self, route_id: u32) {
        self.chrome.minimap.clear_route(route_id as usize);
    }

    // ----- notifications ----------------------------------------------
    //
    // The lifted `Notifications` panel doesn't currently expose a
    // per-toast id (toasts age out via lifetime/fade), so
    // `clear_notification(id)` falls back to clearing the hover
    // pause state. The id is accepted for forward-compat with
    // the desktop's id-keyed toast removal so the host doesn't have
    // to change its call site when the panel grows real ids.

    /// Append one toast. JSON shape:
    ///
    /// ```text
    /// {
    ///   "message": String,              // required
    ///   "severity": "info" | "warn" | "error",  // case-insensitive
    ///   "title": String,                // optional, prepended as "title — message"
    /// }
    /// ```
    ///
    /// Unknown severity values fall back to info — matches the
    /// desktop's lenient mapping. Empty `message` after combining
    /// with `title` is dropped (matches the panel's own
    /// empty-message guard).
    pub fn push_notification(&mut self, notification_json: &str) -> Result<(), JsValue> {
        use neoism_ui::panels::notifications::NotificationLevel;
        #[derive(serde::Deserialize)]
        struct JsNotification {
            #[serde(default)]
            message: String,
            #[serde(default)]
            severity: String,
            #[serde(default)]
            title: String,
        }
        let parsed: JsNotification = serde_json::from_str(notification_json)
            .map_err(|e| JsValue::from_str(&format!("notification parse: {e}")))?;
        let level = match parsed.severity.trim().to_ascii_lowercase().as_str() {
            "error" | "err" => NotificationLevel::Error,
            "warn" | "warning" => NotificationLevel::Warn,
            _ => NotificationLevel::Info,
        };
        let body = if parsed.title.is_empty() {
            parsed.message
        } else if parsed.message.is_empty() {
            parsed.title
        } else {
            format!("{} — {}", parsed.title, parsed.message)
        };
        self.chrome.notifications.push(body, level);
        Ok(())
    }

    /// Forward-compat dismiss-by-id stub. Today the shared panel has
    /// no id keying, so we clear the hover state (the closest
    /// available no-op-on-empty operation). When the panel grows
    /// real ids, route through them here.
    pub fn clear_notification(&mut self, _id: u32) {
        self.chrome.notifications.clear_hover();
    }

    /// Consume workspace-daemon pushes that affect chrome-owned
    /// state. The JS side still handles most workspace effects
    /// directly; this keeps the wasm bridge in sync for editor
    /// surface bindings and gives protocol errors a visible toast.
    pub fn workspace_event(&mut self, event_json: &str) -> Result<(), JsValue> {
        let event: WorkspaceServerMessage = serde_json::from_str(event_json)
            .map_err(|e| JsValue::from_str(&format!("workspace event parse: {e}")))?;
        match event {
            WorkspaceServerMessage::EditorSurfaceList { surfaces } => {
                self.editor_surfaces = surfaces;
            }
            WorkspaceServerMessage::EditorSurfaceChanged { surface } => {
                if let Some(existing) = self
                    .editor_surfaces
                    .iter_mut()
                    .find(|existing| existing.surface_id == surface.surface_id)
                {
                    *existing = surface;
                } else {
                    self.editor_surfaces.push(surface);
                }
            }
            WorkspaceServerMessage::EditorSurfaceClosed { surface_id } => {
                self.editor_surfaces
                    .retain(|surface| surface.surface_id != surface_id);
            }
            WorkspaceServerMessage::Error { message } => {
                self.chrome.notifications.push(
                    message,
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
            }
            WorkspaceServerMessage::WorkspaceActionCompleted { message, .. } => {
                self.chrome.notifications.push(
                    message,
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
            _ => {}
        }
        Ok(())
    }

    // ----- diagnostics popup ------------------------------------------
    //
    // Wire-shape: JSON array of `{ line, col, severity, message,
    // source? }` records (mirrors `neoism_protocol::diagnostics::DiagnosticItem`).
    // `severity` is the nvim integer code (1=Error, 2=Warn, 3=Info,
    // 4=Hint) — translated to the panel's `Severity` enum via
    // `DiagnosticSeverity::from_u8`. `line` may be 0-based (per
    // protocol) — we widen to 1-based for the panel's `lnum` so the
    // displayed row matches nvim's `:<lnum>` jump convention.
    //
    // `set_diagnostics` caches the items + refreshes the popup
    // when visible; `show_diagnostics_at(line, col)` opens the
    // popup anchored at the cell `(col, line)` using the current
    // `cell_w`/`cell_h`; `hide_diagnostics` closes it.

    /// Replace the cached diagnostics list. JSON shape:
    ///
    /// ```text
    /// [
    ///   {
    ///     "line": u32,         // 0-based row (protocol convention)
    ///     "col": u32,          // 0-based column
    ///     "severity": u8,      // 1=Error, 2=Warn, 3=Info, 4=Hint
    ///     "message": String,
    ///     "source": Option<String>
    ///   },
    ///   ...
    /// ]
    /// ```
    ///
    /// The list is cached on the bridge so a subsequent
    /// `show_diagnostics_at(...)` opens the popup with the latest
    /// items; when the popup is already visible the items are
    /// refreshed in place via `DiagnosticsPopup::refresh_items`.
    pub fn set_diagnostics(&mut self, items_json: &str) -> Result<(), JsValue> {
        #[derive(serde::Deserialize)]
        struct JsDiag {
            #[serde(default)]
            line: u32,
            #[serde(default)]
            severity: u8,
            #[serde(default)]
            message: String,
            #[serde(default)]
            #[allow(dead_code)]
            source: Option<String>,
        }
        let parsed: Vec<JsDiag> = serde_json::from_str(items_json)
            .map_err(|e| JsValue::from_str(&format!("diagnostics parse: {e}")))?;
        let items: Vec<neoism_ui::panels::diagnostics_popup::PopupItem> =
            parsed
                .into_iter()
                .map(|d| {
                    let sev = neoism_ui::editor_snapshot::DiagnosticSeverity::from_u8(
                        d.severity,
                    );
                    neoism_ui::panels::diagnostics_popup::PopupItem {
                        // `line` is 0-based on the wire; `lnum` is
                        // 1-based to match the panel's row display.
                        lnum: (d.line as u64).saturating_add(1),
                        severity:
                            neoism_ui::panels::diagnostics_popup::Severity::from_snapshot(
                                sev,
                            ),
                        message: d.message,
                    }
                })
                .collect();
        // Stash the latest items so `show_diagnostics_at(...)`
        // opens with the live list; refresh in place when already
        // visible.
        self.cached_diagnostics = items.clone();
        if self.chrome.diagnostics_popup.is_visible() {
            self.chrome.diagnostics_popup.refresh_items(items);
        }
        Ok(())
    }

    /// Open the diagnostics popup anchored at the editor cell
    /// `(col, line)`. `line`/`col` are 0-based grid cells; the
    /// bridge multiplies by the current `cell_w`/`cell_h` to get
    /// physical pixel anchor coords. Picks the `Error` pill when
    /// any cached item is an error, else `Warn` — keeps the popup
    /// header's color/label aligned with the highest severity.
    pub fn show_diagnostics_at(&mut self, line: u32, col: u32) {
        use neoism_ui::panels::diagnostics_popup::Severity;
        use neoism_ui::panels::status_line::DiagnosticPill;
        let items = self.cached_diagnostics.clone();
        let pill = if items.iter().any(|i| i.severity == Severity::Error) {
            DiagnosticPill::Error
        } else {
            DiagnosticPill::Warn
        };
        let cw = self.chrome.cell_metrics().0.max(1.0);
        let ch = self.chrome.cell_metrics().1.max(1.0);
        let ax = (col as f32) * cw;
        let ay = (line as f32) * ch;
        self.chrome.diagnostics_popup.open(pill, items, ax, ay);
    }

    /// Close the diagnostics popup. The cached items survive so a
    /// subsequent `show_diagnostics_at(...)` re-opens with the
    /// same snapshot.
    pub fn hide_diagnostics(&mut self) {
        self.chrome.diagnostics_popup.close();
    }

    /// Hit-test a status-line / diagnostics-popup click in chrome
    /// logical pixels and return the host action to execute. This
    /// mirrors desktop `Screen::handle_diagnostics_click`: popup
    /// row clicks jump to a diagnostic line, outside clicks close
    /// the popup and fall through to status pills, branch clicks
    /// open the git diff panel, and diagnostic pills open the
    /// shared diagnostics popup.
    pub fn status_line_click(&mut self, x: f32, y: f32) -> JsValue {
        use neoism_ui::panels::diagnostics_popup::Severity;
        use neoism_ui::panels::status_line::StatusLineClickAction;

        if self.chrome.diagnostics_popup.is_visible() {
            match self.chrome.diagnostics_popup.hit_test(x, y) {
                Ok(Some(idx)) => {
                    if self.chrome.diagnostics_popup.is_interactive() {
                        self.chrome.diagnostics_popup.set_selected_index(idx);
                        let line = self.chrome.diagnostics_popup.selected_lnum();
                        self.chrome.diagnostics_popup.close();
                        if let Some(line) = line {
                            return serde_wasm_bindgen::to_value(
                                &StatusLineClickIntent::DiagnosticJump { line },
                            )
                            .unwrap_or(JsValue::NULL);
                        }
                    }
                    return serde_wasm_bindgen::to_value(
                        &StatusLineClickIntent::Consumed,
                    )
                    .unwrap_or(JsValue::NULL);
                }
                Ok(None) => {
                    return serde_wasm_bindgen::to_value(
                        &StatusLineClickIntent::Consumed,
                    )
                    .unwrap_or(JsValue::NULL);
                }
                Err(()) => {
                    self.chrome.diagnostics_popup.close();
                }
            }
        }

        let Some(action) = self.chrome.status_line.click_action_at(x, y) else {
            return JsValue::NULL;
        };

        let intent = match action {
            StatusLineClickAction::ToggleSplit => StatusLineClickIntent::ToggleSplit,
            StatusLineClickAction::ToggleGitDiff => StatusLineClickIntent::ToggleGitDiff,
            StatusLineClickAction::ToggleLspPopup => StatusLineClickIntent::Consumed,
            StatusLineClickAction::Diagnostics { pill } => {
                let items: Vec<_> = self
                    .cached_diagnostics
                    .iter()
                    .filter(|item| match pill {
                        neoism_ui::panels::status_line::DiagnosticPill::Error => {
                            item.severity == Severity::Error
                        }
                        neoism_ui::panels::status_line::DiagnosticPill::Warn => {
                            item.severity == Severity::Warn
                        }
                    })
                    .cloned()
                    .collect();
                if let Some((ax, ay)) =
                    self.chrome.status_line.diagnostic_pill_anchor(pill)
                {
                    self.chrome.diagnostics_popup.open(pill, items, ax, ay);
                    StatusLineClickIntent::DiagnosticsOpened
                } else {
                    StatusLineClickIntent::Consumed
                }
            }
        };
        serde_wasm_bindgen::to_value(&intent).unwrap_or(JsValue::NULL)
    }

    // ----- context menu -----------------------------------------------
    //
    // The web frontend has no real daemon-side source for context
    // menus today (right-click is a host-driven affair). These
    // setters exist so a host that wants to surface a generic
    // menu — e.g. the markdown link-completion menu fed from the
    // chrome's own click handling — can do so through the same
    // state-push pattern as the rest of the panels. Items carry
    // text + hint only; the action is stamped as `ModalAction::Close`
    // because dispatch lives entirely in the host (it watches the
    // popover for selection via JS-side hover tracking and runs
    // its own logic).

    /// Open the context menu. JSON shape:
    ///
    /// ```text
    /// {
    ///   "title": String,                    // empty = no header
    ///   "x": f32,                           // anchor, phys px
    ///   "y": f32,
    ///   "window_w": f32,                    // for clamp inside vp
    ///   "window_h": f32,
    ///   "items": [
    ///     { "label": String, "hint": String, "enabled": bool },
    ///     ...
    ///   ]
    /// }
    /// ```
    ///
    /// Items use `ModalAction::Close` as a sentinel action — the
    /// menu's render is text-only on web; selection routing is
    /// done by the host via `chrome_layout`/event capture, not by
    /// the menu's action enum. Empty `items` closes the menu
    /// instantly.
    pub fn set_context_menu(&mut self, payload_json: &str) -> Result<(), JsValue> {
        #[derive(serde::Deserialize)]
        struct JsItem {
            #[serde(default)]
            label: String,
            #[serde(default)]
            hint: String,
            #[serde(default = "default_true_ctx")]
            enabled: bool,
        }
        #[derive(serde::Deserialize)]
        struct JsCtxMenu {
            #[serde(default)]
            title: String,
            #[serde(default)]
            x: f32,
            #[serde(default)]
            y: f32,
            #[serde(default = "default_window_w")]
            window_w: f32,
            #[serde(default = "default_window_h")]
            window_h: f32,
            #[serde(default)]
            items: Vec<JsItem>,
        }
        fn default_true_ctx() -> bool {
            true
        }
        fn default_window_w() -> f32 {
            1024.0
        }
        fn default_window_h() -> f32 {
            768.0
        }
        let parsed: JsCtxMenu = serde_json::from_str(payload_json)
            .map_err(|e| JsValue::from_str(&format!("context_menu parse: {e}")))?;
        let items: Vec<neoism_ui::panels::context_menu::ContextMenuItem> = parsed
            .items
            .into_iter()
            .map(|it| {
                let mut ci = neoism_ui::panels::context_menu::ContextMenuItem::new(
                    it.label,
                    it.hint,
                    // The web bridge doesn't dispatch the
                    // structured action enum — `ModalAction::Close`
                    // is a benign sentinel that simply means "the
                    // menu wants to be dismissed when chosen".
                    neoism_ui::panels::context_menu::ContextMenuAction::Modal(
                        neoism_ui::widgets::modal::ModalAction::Close,
                    ),
                );
                ci.enabled = it.enabled;
                ci
            })
            .collect();
        self.chrome.context_menu.open(
            parsed.title,
            items,
            parsed.x,
            parsed.y,
            parsed.window_w,
            parsed.window_h,
        );
        Ok(())
    }

    /// Hide the context menu. Idempotent — safe to call when the
    /// menu is already closed.
    pub fn hide_context_menu(&mut self) {
        self.chrome.context_menu.close();
    }

    // ----- git branch pill --------------------------------------------
    //
    // GAP: there is no dedicated `git_branch` panel struct in the
    // shared crate — `neoism_ui::panels::git_branch` is a free-
    // function module (process-spawn git CLI helpers, native-only).
    // The branch pill rendered in the status strip is part of
    // `StatusLine` and is populated via `set_status_branch` /
    // `set_status_git_changes`. We expose a single combined setter
    // that writes both fields so the host has a one-call surface
    // matching the W3-B spec; ahead/behind/dirty are encoded into
    // the displayed label until a dedicated panel is lifted in.

    /// Populate the status-line's branch pill. `name = None` clears
    /// the pill entirely. `ahead`/`behind`/`dirty` are encoded into
    /// the displayed label (e.g. `main \u{2191}2 \u{2193}1*`) until
    /// the shared `StatusInfo` grows dedicated fields.
    pub fn set_git_branch_pill(
        &mut self,
        name: Option<String>,
        ahead: u32,
        behind: u32,
        dirty: bool,
    ) {
        let label = name.as_ref().map(|raw| {
            let mut s = raw.clone();
            if ahead > 0 {
                s.push_str(&format!(" \u{2191}{ahead}"));
            }
            if behind > 0 {
                s.push_str(&format!(" \u{2193}{behind}"));
            }
            if dirty {
                s.push('*');
            }
            s
        });
        let mut info = self.chrome.status_line.info().clone();
        info.branch = label;
        self.chrome.status_line.set_info(info);
    }

    // ----- cursor overlays --------------------------------------------
    //
    // Four state-push surfaces for the cursor-trail, custom mouse
    // cursor sprite, animated cursorline rectangle, and yank-flash
    // overlay. The desktop renderer drives these locally from its
    // editor / mouse loop; the web bridge has no equivalent host
    // hook, so JS pushes server-resolved cursor state through these
    // methods. Wire-shapes are documented inline alongside each
    // setter — they mirror the underlying panel API but live in
    // this crate (not `neoism-protocol`) because the bridge
    // boundary is host↔chrome, not daemon↔frontend.

    /// Return the chrome's current cell metrics as
    /// `[cell_w, cell_h]` (physical pixels). The web dispatcher
    /// reads these to translate daemon-side cell coordinates
    /// (`CursorOverlayServerMessage::TrailCursor.col/row`,
    /// `CursorlineOverlay.target_row`) into the physical-pixel
    /// `x`/`y`/`target_y` the underlying setters expect. Returned
    /// as a `Vec<f32>` so the JS side can index `[0]`/`[1]`
    /// without an extra `js_sys::Array` allocation.
    pub fn cell_metrics(&self) -> Vec<f32> {
        let (cw, ch) = self.chrome.cell_metrics();
        vec![cw.max(1.0), ch.max(1.0)]
    }

    /// Push the trail cursor's latest destination. JSON shape:
    ///
    /// ```text
    /// {
    ///   "x": f32,                  // top-left of cell (phys px)
    ///   "y": f32,
    ///   "cell_w": f32,             // cell metrics (phys px)
    ///   "cell_h": f32,
    ///   "shape": "block" | "beam" | "underline" | "hidden",
    ///   "no_jump": bool,           // optional, defaults false
    ///   "reset": bool,             // optional, defaults false
    ///   "snap": bool               // optional, defaults false
    /// }
    /// ```
    ///
    /// `reset = true` clears the spring + last-destination cache
    /// (use this when the active pane switches). `snap = true`
    /// teleports the trail to the destination without animating
    /// (mirrors `TrailCursor::snap_to_destination`). `no_jump =
    /// true` updates the destination without marking a new logical
    /// cursor jump — matches the scroll-spring follow path so the
    /// trail follows a sliding viewport without re-ranking corners.
    pub fn set_trail_cursor(&mut self, json: &str) -> Result<(), JsValue> {
        use neoism_terminal_core::ansi::CursorShape;
        #[derive(serde::Deserialize)]
        struct JsTrailCursor {
            #[serde(default)]
            x: f32,
            #[serde(default)]
            y: f32,
            #[serde(default = "default_cell_w")]
            cell_w: f32,
            #[serde(default = "default_cell_h")]
            cell_h: f32,
            #[serde(default)]
            shape: Option<String>,
            #[serde(default)]
            no_jump: bool,
            #[serde(default)]
            reset: bool,
            #[serde(default)]
            snap: bool,
        }
        fn default_cell_w() -> f32 {
            8.0
        }
        fn default_cell_h() -> f32 {
            16.0
        }

        let parsed: JsTrailCursor = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("trail_cursor parse: {e}")))?;

        if parsed.reset {
            self.chrome.trail_cursor.reset();
        }

        let shape = match parsed.shape.as_deref().map(str::to_ascii_lowercase) {
            Some(ref s) if s == "beam" => CursorShape::Beam,
            Some(ref s) if s == "underline" => CursorShape::Underline,
            Some(ref s) if s == "hidden" => CursorShape::Hidden,
            _ => CursorShape::Block,
        };
        self.chrome.trail_cursor.set_cursor_shape(shape);

        let cw = parsed.cell_w.max(1.0);
        let ch = parsed.cell_h.max(1.0);

        if parsed.no_jump {
            self.chrome
                .trail_cursor
                .set_destination_no_jump(parsed.x, parsed.y, cw, ch);
        } else {
            self.chrome
                .trail_cursor
                .set_destination(parsed.x, parsed.y, cw, ch);
        }

        if parsed.snap {
            self.chrome.trail_cursor.snap_to_destination(cw, ch);
        }
        Ok(())
    }

    /// Push the custom mouse-cursor sprite position. JSON shape:
    ///
    /// ```text
    /// {
    ///   "x": f32,                  // pointer pos, phys px
    ///   "y": f32,
    ///   "visible": bool            // optional, defaults true
    /// }
    /// ```
    ///
    /// `visible = false` hides the sprite without forgetting the
    /// last-known position — use that when the pointer leaves the
    /// canvas so the sprite doesn't ghost in the corner.
    pub fn set_custom_cursor(&mut self, json: &str) -> Result<(), JsValue> {
        #[derive(serde::Deserialize)]
        struct JsCustomCursor {
            #[serde(default)]
            x: f32,
            #[serde(default)]
            y: f32,
            #[serde(default = "default_true")]
            visible: bool,
        }
        fn default_true() -> bool {
            true
        }

        let parsed: JsCustomCursor = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("custom_cursor parse: {e}")))?;

        self.chrome
            .custom_cursor
            .set_position(parsed.x, parsed.y, parsed.visible);
        Ok(())
    }

    /// Push the cursorline-overlay target for one editor pane.
    /// JSON shape:
    ///
    /// ```text
    /// {
    ///   "rich_text_id": u32,       // 0-based pane / rich-text id
    ///   "target_y": f32,           // top of highlighted row (phys px)
    ///   "snap": bool,              // optional, defaults false
    ///   "forget": bool             // optional, defaults false
    /// }
    /// ```
    ///
    /// `snap = true` pins the highlight to `target_y` with no glide
    /// (mirrors the scroll-spring-active path in
    /// `CursorlineOverlay::set_target`). `forget = true` drops the
    /// cached state for the pane id — call when the pane is
    /// closed/destroyed.
    pub fn set_cursorline_overlay(&mut self, json: &str) -> Result<(), JsValue> {
        #[derive(serde::Deserialize)]
        struct JsCursorline {
            #[serde(default)]
            rich_text_id: u32,
            #[serde(default)]
            target_y: f32,
            #[serde(default)]
            snap: bool,
            #[serde(default)]
            forget: bool,
        }

        let parsed: JsCursorline = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("cursorline parse: {e}")))?;

        let id = parsed.rich_text_id as usize;
        if parsed.forget {
            self.chrome.cursorline_overlay.forget(id);
            return Ok(());
        }
        self.chrome
            .cursorline_overlay
            .set_target(id, parsed.target_y, parsed.snap);
        Ok(())
    }

    /// Push a yank-flash region. JSON shape:
    ///
    /// ```text
    /// {
    ///   "regions": [
    ///     { "row_top": u32, "row_bot": u32, "col_left"?: u32, "col_right"?: u32 },
    ///     ...
    ///   ]
    /// }
    /// ```
    ///
    /// `regions` is an array so a single push can spawn the
    /// multi-line flash a visual-block yank emits. Empty array is
    /// a no-op. Each region's `row_top` / `row_bot` are 0-based
    /// screen rows relative to the editor pane top (matches
    /// `YankFlash::push`). The flash fades over ~360ms — no
    /// follow-up `clear` call is required.
    pub fn set_yank_flash(&mut self, json: &str) -> Result<(), JsValue> {
        #[derive(serde::Deserialize)]
        struct JsYankRegion {
            #[serde(default)]
            row_top: u32,
            #[serde(default)]
            row_bot: u32,
            #[serde(default)]
            col_left: Option<u32>,
            #[serde(default)]
            col_right: Option<u32>,
        }
        #[derive(serde::Deserialize)]
        struct JsYankFlash {
            #[serde(default)]
            regions: Vec<JsYankRegion>,
        }

        let parsed: JsYankFlash = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("yank_flash parse: {e}")))?;

        for region in parsed.regions {
            self.chrome.yank_flash.push_span(
                region.row_top,
                region.row_bot,
                region.col_left,
                region.col_right,
            );
        }
        Ok(())
    }
}
