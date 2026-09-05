use super::*;
use neoism_ui::panels::buffer_tabs::{
    apply_buffer_tab_policy as shared_apply_buffer_tab_policy, BufferTabPolicyInput,
    BufferTabPolicyOperation, TabHit,
};
use neoism_ui::panels::pane_grid::PaneGrid;
use neoism_ui::session_layout::tree::{SessionTree, VisualDir};
use neoism_ui::session_layout::{
    SessionLeafKind, SessionLeafSpec, SplitAxis, SplitPlacement,
};

/// One visible pane in unit space (`x`/`y`/`w`/`h` ∈ [0, 1] relative to
/// the content rect) — the shape the web host's dispatcher consumes.
#[derive(serde::Serialize)]
struct WebPaneRect {
    external_id: u64,
    leaf_id: u64,
    kind: String,
    title: Option<String>,
    focused: bool,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(serde::Serialize)]
struct WebSessionLayoutPolicyResult {
    state_json: String,
    focused_external_id: Option<u64>,
    active_external_ids: Vec<u64>,
    panes: Vec<WebPaneRect>,
    changed: bool,
}

fn web_leaf_kind_str(kind: &SessionLeafKind) -> String {
    match kind {
        SessionLeafKind::Terminal => "terminal".to_string(),
        SessionLeafKind::Editor => "editor".to_string(),
        SessionLeafKind::Agent => "agent".to_string(),
        SessionLeafKind::Custom(kind) => kind.clone(),
    }
}

/// Solve `tree` in unit space and package the policy result the JS
/// host expects. Pane rects come from the SAME shared geometry solver
/// (`session_layout::geometry::solve`) desktop's grid mirrors, so the
/// web's normalized rects can't drift from the canonical model.
fn web_tree_policy_result(
    tree: &SessionTree,
    changed: bool,
) -> Result<WebSessionLayoutPolicyResult, JsValue> {
    let solved = neoism_ui::session_layout::geometry::solve(
        tree,
        neoism_ui::layout::Rect::new(0.0, 0.0, 1.0, 1.0),
        0.0,
        0.0,
    );
    let panes = solved
        .panes
        .iter()
        .filter_map(|pane| {
            let external_id = pane.external_id?;
            Some(WebPaneRect {
                external_id,
                leaf_id: pane.leaf.0,
                kind: web_leaf_kind_str(&pane.kind),
                title: tree.leaf(pane.leaf).and_then(|leaf| leaf.title.clone()),
                focused: pane.focused,
                x: pane.rect.x,
                y: pane.rect.y,
                w: pane.rect.w,
                h: pane.rect.h,
            })
        })
        .collect();
    let state_json = serde_json::to_string(tree)
        .map_err(|e| JsValue::from_str(&format!("layout serialize: {e}")))?;
    Ok(WebSessionLayoutPolicyResult {
        focused_external_id: tree.leaf(tree.focus()).and_then(|leaf| leaf.external_id),
        // ALL leaf external ids (hidden tab siblings included) so the
        // host's id allocator can never collide with a parked leaf.
        active_external_ids: tree.external_ids(),
        panes,
        changed,
        state_json,
    })
}

#[wasm_bindgen]
impl ChromeBridge {
    /// Return the tab index under a window-space logical pixel, or -1 when
    /// the point is outside a tab (including the trailing new-tab button).
    /// The web host uses this to begin a native-feeling pointer drag while
    /// the shared BufferTabs remains the hit-test authority.
    pub fn buffer_tab_hit_test(&self, x: f32, y: f32) -> i32 {
        let rect = self.chrome.layout().buffer_tabs;
        match self
            .chrome
            .buffer_tabs
            .hit_test(x, y, rect.x, rect.y, rect.w)
        {
            Some(TabHit::Activate(index)) => index as i32,
            _ => -1,
        }
    }

    /// Replace the buffer-tab strip with the given titles, marking
    /// `active` as the selected tab. JS uses this after the tree
    /// triggers an open: it appends a new tab to its own bookkeeping
    /// list and replays the full set so the panel reflects current
    /// state without exposing the generic `A` parameter.
    pub fn set_buffer_tabs(
        &mut self,
        tabs_json: &str,
        active: u32,
    ) -> Result<(), JsValue> {
        use neoism_ui::panels::buffer_tabs::BufferTab;
        #[derive(serde::Deserialize)]
        struct JsTab {
            title: String,
            #[serde(default)]
            modified: bool,
            #[serde(default)]
            path: Option<String>,
            #[serde(default)]
            kind: Option<String>,
            #[serde(default)]
            neoism_agent_route_id: Option<usize>,
        }
        // Backwards-compat: the old shape was `Vec<String>` of bare
        // titles. Accept either shape so a stale JS bundle doesn't
        // brick the bridge.
        let raw: Vec<JsTab> = match serde_json::from_str::<Vec<JsTab>>(tabs_json) {
            Ok(v) => v,
            Err(_) => serde_json::from_str::<Vec<String>>(tabs_json)
                .map(|titles| {
                    titles
                        .into_iter()
                        .map(|title| JsTab {
                            title,
                            modified: false,
                            path: None,
                            kind: None,
                            neoism_agent_route_id: None,
                        })
                        .collect()
                })
                .map_err(|e| JsValue::from_str(&format!("tabs parse: {e}")))?,
        };
        self.tab_kinds.clear();
        for (ix, t) in raw.iter().enumerate() {
            if let Some(kind) = t.kind.as_deref() {
                self.tab_kinds.insert(ix, kind.to_string());
            } else if t.path.is_some() {
                self.tab_kinds.insert(ix, "file".to_string());
            } else {
                self.tab_kinds.insert(ix, "terminal".to_string());
            }
        }
        self.tab_paths.retain(|ix, path| {
            raw.get(*ix)
                .and_then(|tab| tab.path.as_ref())
                .is_some_and(|current| current == path)
        });
        self.tab_contents
            .retain(|ix, _| self.tab_paths.contains_key(ix));
        let tabs: Vec<BufferTab<()>> = raw
            .into_iter()
            .enumerate()
            .map(|(ix, t)| {
                let agent_route = t.neoism_agent_route_id.or_else(|| {
                    (t.kind.as_deref() == Some("neoism-agent")).then_some(ix)
                });
                // Chrome helper-page tabs (Extensions / NeoWorld) ride
                // the same JS-owned replay as every other tab; the
                // kind string maps onto the shared ChromePageRef so
                // `Chrome::active_chrome_page` resolves and the page
                // body paints (desktop's open_chrome_page twin).
                let chrome_page = match t.kind.as_deref() {
                    Some("chrome-extensions") => {
                        Some(neoism_ui::panels::buffer_tabs::ChromePageRef {
                            kind: neoism_ui::panels::buffer_tabs::ChromePageKind::Extensions,
                            route_id: ix,
                        })
                    }
                    Some("chrome-neoworld") => {
                        Some(neoism_ui::panels::buffer_tabs::ChromePageRef {
                            kind: neoism_ui::panels::buffer_tabs::ChromePageKind::NeoWorld,
                            route_id: ix,
                        })
                    }
                    _ => None,
                };
                BufferTab {
                    title: t.title,
                    modified: t.modified,
                    custom_icon: None,
                    // A tab with no `path` (and no scratch/agent route)
                    // is treated as the root terminal — sticky, no close
                    // button. File tabs MUST carry their path so the
                    // panel paints the X. Neoism Agent tabs carry their
                    // route id instead; the desktop frontend's
                    // `NeoismAgentPane` paints the contents.
                    path: agent_route
                        .is_none()
                        .then(|| t.path.as_deref().map(std::path::PathBuf::from))
                        .flatten(),
                    markdown: t
                        .path
                        .as_deref()
                        .map(|p| {
                            neoism_ui::syntax::Lang::from_path(p)
                                == neoism_ui::syntax::Lang::Markdown
                        })
                        .unwrap_or(false),
                    terminal_route_id: (t.kind.as_deref() == Some("terminal") && ix != 0)
                        .then_some(ix),
                    neoism_agent_route_id: agent_route,
                    chrome_page,
                    agent_kind: None,
                }
            })
            .collect();
        let active_idx = (active as usize).min(tabs.len().saturating_sub(1));
        self.chrome.buffer_tabs.set_visible(!tabs.is_empty());
        self.chrome.buffer_tabs.set_tabs(tabs, active_idx);
        self.sync_active_tab_state(active_idx);
        self.sync_status_mode_for_active_tab_index();
        self.relayout_chrome();
        Ok(())
    }

    /// Apply shared buffer-tab operation policy for JS-owned tabs.
    ///
    /// JS still owns web-only side effects such as closing PTY sessions
    /// and replaying inactive terminal buffers. This returns the shared
    /// bookkeeping decision so web selection/reorder/close behavior stays
    /// aligned with the Rust panel model.
    pub fn apply_buffer_tab_policy(
        &self,
        tabs_json: &str,
        active: u32,
        operation: &str,
        index: Option<u32>,
    ) -> Result<JsValue, JsValue> {
        #[derive(serde::Deserialize)]
        struct JsTab {
            #[serde(default)]
            path: Option<String>,
            #[serde(default)]
            kind: Option<String>,
            #[serde(default, alias = "sessionId")]
            session_id: Option<String>,
            #[serde(default, alias = "neoismAgentRouteId")]
            neoism_agent_route_id: Option<usize>,
        }

        let raw: Vec<JsTab> = serde_json::from_str(tabs_json)
            .map_err(|e| JsValue::from_str(&format!("tabs parse: {e}")))?;
        let len = raw.len();
        let closeable = raw
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                let is_agent = tab.neoism_agent_route_id.is_some()
                    || tab.kind.as_deref() == Some("neoism-agent");
                // Chrome helper pages are path-less but NOT the sticky
                // root terminal — they close like any file tab.
                let is_chrome_page = matches!(
                    tab.kind.as_deref(),
                    Some("chrome-extensions") | Some("chrome-neoworld")
                );
                let is_terminal = tab.kind.as_deref() == Some("terminal")
                    || (tab.path.is_none() && !is_agent && !is_chrome_page);
                if is_terminal {
                    ix != 0 && len > 1 && tab.session_id.is_some()
                } else {
                    true
                }
            })
            .collect();
        let operation = match operation {
            "select_previous" => BufferTabPolicyOperation::SelectPrevious,
            "select_next" => BufferTabPolicyOperation::SelectNext,
            "select_index" => BufferTabPolicyOperation::SelectIndex {
                index: index.unwrap_or(0) as usize,
            },
            "move_previous" => BufferTabPolicyOperation::MovePrevious,
            "move_next" => BufferTabPolicyOperation::MoveNext,
            "close_active" => BufferTabPolicyOperation::CloseActive,
            "close_index" => BufferTabPolicyOperation::CloseIndex {
                index: index.unwrap_or(0) as usize,
            },
            "reorder" => {
                let packed = index
                    .ok_or_else(|| JsValue::from_str("reorder requires packed index"))?;
                let from = (packed >> 16) as usize;
                let to = (packed & 0xffff) as usize;
                BufferTabPolicyOperation::Reorder { from, to }
            }
            other => {
                return Err(JsValue::from_str(&format!(
                    "unknown buffer-tab policy operation: {other}"
                )));
            }
        };
        let result = shared_apply_buffer_tab_policy(
            BufferTabPolicyInput {
                len,
                active: active as usize,
                closeable,
            },
            operation,
        );
        serde_wasm_bindgen::to_value(&result)
            .map_err(|e| JsValue::from_str(&format!("policy result: {e}")))
    }

    /// Apply shared session-tree policy for JS-owned visible pane state.
    ///
    /// Web still owns DOM/PTY/nvim side effects. This bridge keeps the
    /// split/focus/close/resize tree in the SAME recursive
    /// [`SessionTree`] model the desktop grid renders (the legacy
    /// two-level `SessionLayout` is desktop-compat only now), then
    /// returns computed normalized rectangles for the browser
    /// dispatcher. Every applied operation also re-seeds the chrome's
    /// shared `PaneGrid` so dividers / drop zones / pane hit tests run
    /// against the identical tree.
    pub fn apply_session_layout_policy(
        &mut self,
        state_json: Option<String>,
        operation: &str,
        axis: Option<String>,
        title: Option<String>,
        external_id: Option<u32>,
    ) -> Result<JsValue, JsValue> {
        fn leaf_kind(raw: Option<&str>) -> SessionLeafKind {
            match raw {
                Some("terminal") => SessionLeafKind::Terminal,
                Some("agent") | Some("neoism-agent") => SessionLeafKind::Agent,
                Some(other) if other != "editor" => {
                    SessionLeafKind::Custom(other.to_string())
                }
                _ => SessionLeafKind::Editor,
            }
        }

        fn split_axis(raw: Option<&str>) -> SplitAxis {
            match raw {
                Some("vertical") | Some("down") | Some("up") => SplitAxis::Vertical,
                _ => SplitAxis::Horizontal,
            }
        }

        fn spec_for(
            kind: Option<&str>,
            title: Option<String>,
            external_id: Option<u32>,
        ) -> SessionLeafSpec {
            let mut spec = SessionLeafSpec::new(leaf_kind(kind));
            if let Some(title) = title.filter(|title| !title.is_empty()) {
                spec = spec.with_title(title);
            }
            if let Some(external_id) = external_id {
                spec = spec.with_external_id(external_id as u64);
            }
            spec
        }

        let mut tree = if let Some(json) = state_json.filter(|json| !json.is_empty()) {
            serde_json::from_str::<SessionTree>(&json)
                .map_err(|e| JsValue::from_str(&format!("layout parse: {e}")))?
        } else {
            let initial_kind = if operation == "init_terminal" {
                "terminal"
            } else {
                "editor"
            };
            SessionTree::new(spec_for(
                Some(initial_kind),
                title.clone().or_else(|| Some("Editor 1".to_string())),
                Some(external_id.unwrap_or(1)),
            ))
        };

        let before = serde_json::to_string(&tree)
            .map_err(|e| JsValue::from_str(&format!("layout snapshot: {e}")))?;

        match operation {
            "init" | "init_editor" | "init_terminal" => {}
            "split" | "split_before" | "split_terminal" | "split_terminal_before" => {
                // Explicit terminal-split ops mint a terminal leaf;
                // plain splits inherit the focused leaf's surface kind
                // (desktop parity: splitting a terminal opens another
                // terminal, splitting an editor another editor slot).
                let kind = if operation.starts_with("split_terminal") {
                    "terminal"
                } else {
                    match tree.leaf(tree.focus()).map(|l| l.kind.clone()) {
                        Some(SessionLeafKind::Terminal) => "terminal",
                        Some(SessionLeafKind::Agent) => "agent",
                        _ => "editor",
                    }
                };
                tree.split_focused(
                    split_axis(axis.as_deref()),
                    if operation.ends_with("before") {
                        SplitPlacement::Before
                    } else {
                        SplitPlacement::After
                    },
                    spec_for(Some(kind), title, external_id),
                )
                .map_err(|e| JsValue::from_str(&format!("layout split: {e:?}")))?;
            }
            "focus_next" => {
                tree.focus_next_visual(VisualDir::Next)
                    .map_err(|e| JsValue::from_str(&format!("layout focus: {e:?}")))?;
            }
            "focus_prev" => {
                tree.focus_next_visual(VisualDir::Previous)
                    .map_err(|e| JsValue::from_str(&format!("layout focus: {e:?}")))?;
            }
            "focus_external" => {
                let external_id = external_id.ok_or_else(|| {
                    JsValue::from_str("layout focus_external requires external_id")
                })? as u64;
                let leaf = tree
                    .all_leaves()
                    .into_iter()
                    .find(|leaf_id| {
                        tree.leaf(*leaf_id).and_then(|leaf| leaf.external_id)
                            == Some(external_id)
                    })
                    .ok_or_else(|| {
                        JsValue::from_str(&format!(
                            "layout focus_external missing pane {external_id}"
                        ))
                    })?;
                tree.focus_leaf(leaf).map_err(|e| {
                    JsValue::from_str(&format!("layout focus_external: {e:?}"))
                })?;
            }
            // Ensure a leaf exists for `external_id`. No-op if one is
            // already present; otherwise split the focused leaf so a
            // new editor pane appears tagged with that external_id.
            // Used by the web frontend to react to remote
            // `EditorSurfaceChanged` pushes (e.g. neoism-agent on a
            // paired phone binding a brand-new pane in this session).
            "ensure_external" => {
                let external_id = external_id.ok_or_else(|| {
                    JsValue::from_str("layout ensure_external requires external_id")
                })? as u64;
                let already = tree.all_leaves().into_iter().any(|leaf_id| {
                    tree.leaf(leaf_id).and_then(|leaf| leaf.external_id)
                        == Some(external_id)
                });
                if !already {
                    tree.split_focused(
                        split_axis(axis.as_deref()),
                        SplitPlacement::After,
                        spec_for(Some("editor"), title, Some(external_id as u32)),
                    )
                    .map_err(|e| {
                        JsValue::from_str(&format!("layout ensure_external: {e:?}"))
                    })?;
                }
            }
            "close_focused" => {
                tree.close_focused()
                    .map_err(|e| JsValue::from_str(&format!("layout close: {e:?}")))?;
            }
            "resize" => {
                let (axis, delta) = match axis.as_deref() {
                    Some("up") => (SplitAxis::Vertical, -0.05),
                    Some("down") => (SplitAxis::Vertical, 0.05),
                    Some("left") => (SplitAxis::Horizontal, -0.05),
                    Some("right") => (SplitAxis::Horizontal, 0.05),
                    Some("vertical") => (SplitAxis::Vertical, 0.05),
                    _ => (SplitAxis::Horizontal, 0.05),
                };
                tree.resize_event(Some(axis), delta)
                    .map_err(|e| JsValue::from_str(&format!("layout resize: {e:?}")))?;
            }
            other => {
                return Err(JsValue::from_str(&format!(
                    "unknown session-layout policy operation: {other}"
                )));
            }
        }
        tree.validate()
            .map_err(|e| JsValue::from_str(&format!("layout validate: {e:?}")))?;

        let mut result = web_tree_policy_result(&tree, false)?;
        result.changed = before != result.state_json;
        self.adopt_pane_tree(tree);
        serde_wasm_bindgen::to_value(&result)
            .map_err(|e| JsValue::from_str(&format!("layout result: {e}")))
    }

    /// Mirror a daemon `PaneLayoutSnapshot` (the authoritative pane
    /// tree the desktop renders) into the same `SessionTree`-derived
    /// pane rectangles the local `apply_session_layout_policy` path
    /// produces.
    ///
    /// Lowering goes through the shared
    /// `SessionTree::from_pane_layout_snapshot` converter — the exact
    /// inverse of the desktop's snapshot serializer — so the web
    /// renders the split intent (axis, cumulative ratios, nesting, tab
    /// stacks, focus) the desktop mirrors, and the chrome's `PaneGrid`
    /// adopts the identical tree for divider / drop-zone interactions.
    pub fn mirror_pane_layout_snapshot(
        &mut self,
        snapshot_json: &str,
    ) -> Result<JsValue, JsValue> {
        let snapshot = serde_json::from_str::<
            neoism_protocol::workspace::PaneLayoutSnapshot,
        >(snapshot_json)
        .map_err(|e| JsValue::from_str(&format!("snapshot parse: {e}")))?;
        let tree = SessionTree::from_pane_layout_snapshot(&snapshot)
            .map_err(|e| JsValue::from_str(&format!("snapshot mirror: {e:?}")))?;

        let result = web_tree_policy_result(&tree, true)?;
        self.adopt_pane_tree(tree);
        serde_wasm_bindgen::to_value(&result)
            .map_err(|e| JsValue::from_str(&format!("layout result: {e}")))
    }

    // ----------------------------------------------------------------
    // Shared PaneGrid pointer surface — divider drag, focus-by-click,
    // drag-to-split previews. Window-coordinate twins of desktop's
    // pane pointer routing; TS calls these BEFORE its editor/terminal
    // pointer branches so divider grabs win over caret placement.
    // ----------------------------------------------------------------

    /// Pointer press for the shared pane grid. Bit flags:
    /// 1 = consumed, 2 = divider drag started, 4 = focus moved to the
    /// pane under the cursor (drain actions for the external id),
    /// 8 = a per-pane tab-strip interaction was queued (drain via
    /// `drain_pane_tab_intents`).
    pub fn pane_grid_pointer_down(&mut self, x: f32, y: f32) -> u32 {
        if !self.chrome.pane_grid.is_split() || self.chrome.is_neoism_agent_tab_active() {
            return 0;
        }
        // Per-pane tab strips sit on top of the pane bodies; a strip
        // click must never fall through to divider/focus handling.
        if let Some((pane_id, hit)) = self.chrome.pane_strip_hit(x, y) {
            let (kind, index) = match hit {
                TabHit::Activate(ix) => ("activate", ix),
                TabHit::Close(ix) => ("close", ix),
                TabHit::NewTab => ("new_tab", 0),
            };
            self.pending_pane_tab_intents.push(PaneTabIntent {
                external_id: pane_id,
                kind,
                index,
            });
            return 1 | 8;
        }
        if self.chrome.pane_grid.begin_divider_drag(x, y) {
            return 1 | 2;
        }
        let unfocused_pane_hit = neoism_ui::session_layout::geometry::pane_at(
            self.chrome.pane_grid.solved(),
            x,
            y,
        )
        .is_some_and(|pane| !pane.focused);
        if unfocused_pane_hit && self.chrome.pane_grid.focus_at(x, y) {
            return 1 | 4;
        }
        0
    }

    /// Pointer move for the shared pane grid. Bit flags:
    /// 1 = consumed (a drag is in progress), 2 = layout changed
    /// (re-pull `pane_grid_layout_result`).
    pub fn pane_grid_pointer_move(&mut self, x: f32, y: f32) -> u32 {
        if self.chrome.pane_grid.is_divider_dragging() {
            let changed = self.chrome.pane_grid.update_divider_drag(x, y);
            return 1 | if changed { 2 } else { 0 };
        }
        if self.chrome.pane_grid.is_surface_dragging() {
            let _ = self.chrome.pane_grid.update_surface_drag(x, y);
            return 1;
        }
        0
    }

    /// Pointer release for the shared pane grid. Bit flags:
    /// 1 = consumed, 2 = the tree changed (a surface drop committed —
    /// re-pull `pane_grid_layout_result` and drain actions).
    pub fn pane_grid_pointer_up(&mut self, _x: f32, _y: f32) -> u32 {
        if !self.chrome.pane_grid.is_divider_dragging()
            && !self.chrome.pane_grid.is_surface_dragging()
        {
            return 0;
        }
        let changed = self.chrome.pane_grid.end_drag();
        1 | if changed { 2 } else { 0 }
    }

    /// Begin the preview-only surface drag used while a buffer tab is
    /// dragged toward the pane area (tear-out preview). The Rust
    /// chrome paints the drop-zone highlight; the host still commits
    /// the drop through `apply_session_layout_policy`.
    pub fn pane_grid_begin_tab_drag(&mut self) {
        self.chrome.pane_grid.begin_foreign_surface_drag();
    }

    /// Update the drag-to-split preview to window point `(x, y)`.
    /// Returns true while a drop zone is active under the pointer.
    pub fn pane_grid_drag_preview(&mut self, x: f32, y: f32) -> bool {
        self.chrome.pane_grid.update_surface_drag(x, y).is_some()
    }

    /// Cancel any in-progress pane-grid drag (divider or surface)
    /// without mutating the tree. Clears the drop preview.
    pub fn pane_grid_cancel_drag(&mut self) {
        self.chrome.pane_grid.cancel_drag();
    }

    /// Drain queued host actions out of the shared pane grid as JSON:
    /// `[{kind: "focus_pane"|"close_pane", external_id} |
    ///   {kind: "open_pane", leaf_id, leaf_kind} | {kind: "relayout"}]`.
    pub fn drain_pane_grid_actions(&mut self) -> Result<JsValue, JsValue> {
        use neoism_ui::panels::pane_grid::PaneGridAction;

        #[derive(serde::Serialize)]
        struct JsPaneGridAction {
            kind: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            external_id: Option<u64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            leaf_id: Option<u64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            leaf_kind: Option<String>,
        }

        let actions: Vec<JsPaneGridAction> = self
            .chrome
            .pane_grid
            .take_actions()
            .into_iter()
            .map(|action| match action {
                PaneGridAction::OpenPane { leaf, kind } => JsPaneGridAction {
                    kind: "open_pane",
                    external_id: None,
                    leaf_id: Some(leaf.0),
                    leaf_kind: Some(web_leaf_kind_str(&kind)),
                },
                PaneGridAction::ClosePane { external_id } => JsPaneGridAction {
                    kind: "close_pane",
                    external_id: Some(external_id),
                    leaf_id: None,
                    leaf_kind: None,
                },
                PaneGridAction::FocusPane { external_id } => JsPaneGridAction {
                    kind: "focus_pane",
                    external_id: Some(external_id),
                    leaf_id: None,
                    leaf_kind: None,
                },
                PaneGridAction::Relayout => JsPaneGridAction {
                    kind: "relayout",
                    external_id: None,
                    leaf_id: None,
                    leaf_kind: None,
                },
            })
            .collect();
        serde_wasm_bindgen::to_value(&actions)
            .map_err(|e| JsValue::from_str(&format!("pane grid actions: {e}")))
    }

    /// Current layout of the live pane grid in the SAME result shape
    /// `apply_session_layout_policy` returns. TS calls this after a
    /// pointer interaction mutated the grid (divider drag, drop) so
    /// its round-tripped `state_json` / pane list stay in sync with
    /// the Rust-owned tree.
    pub fn pane_grid_layout_result(&self) -> Result<JsValue, JsValue> {
        let result = web_tree_policy_result(self.chrome.pane_grid.tree(), true)?;
        serde_wasm_bindgen::to_value(&result)
            .map_err(|e| JsValue::from_str(&format!("layout result: {e}")))
    }

    // ----------------------------------------------------------------
    // Per-pane terminal surfaces (live multi-pane rendering).
    // ----------------------------------------------------------------

    /// Pre-render host pass for split pane surfaces. TS calls this
    /// right before `render(now)` each frame so per-pane terminal
    /// cells join the same swapchain flip. While a terminal tab is
    /// focused this is a no-op — the render pass paints the split
    /// panes itself (see the pane hook in
    /// `draw_terminal_blocks_or_cells`).
    pub fn draw_pane_grid_host_surfaces(&mut self) {
        if self.chrome.is_terminal_tab_active()
            && !self.chrome.is_neoism_agent_tab_active()
        {
            return;
        }
        let _ = self.draw_split_terminal_panes();
    }

    /// Push per-pane surface descriptors (what each visible pane
    /// shows) into the chrome so unfocused editor panes can resolve
    /// their parked panes and placeholders get honest labels. JSON:
    /// `[{external_id, kind, path?, title?}]`.
    pub fn set_pane_surfaces(&mut self, json: &str) -> Result<(), JsValue> {
        #[derive(serde::Deserialize)]
        struct JsPaneSurface {
            external_id: u64,
            kind: String,
            #[serde(default)]
            path: Option<String>,
            #[serde(default)]
            title: Option<String>,
        }
        let raw: Vec<JsPaneSurface> = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("pane surfaces parse: {e}")))?;
        self.chrome.set_pane_surfaces(
            raw.into_iter()
                .map(|s| neoism_ui::chrome::PaneSurfaceInfo {
                    external_id: s.external_id,
                    kind: s.kind,
                    path: s.path.map(std::path::PathBuf::from),
                    title: s.title,
                })
                .collect(),
        );
        Ok(())
    }

    /// Feed one pane terminal's PTY stream (creating the pane-sized
    /// terminal on first feed, seeded with the active IdeTheme). PTY
    /// Returns effects to the host so replies and private Neoism OSC commands
    /// are routed to the source pane/session rather than whichever tab later
    /// owns focus.
    pub fn feed_pane_terminal(&mut self, external_id: u32, bytes: &[u8]) -> JsValue {
        let id = external_id as u64;
        let cell_w = self.rendered.cell_w.max(1.0);
        let cell_h = self.rendered.cell_h.max(1.0);
        let rect = self.chrome.pane_content_rect(id).or_else(|| {
            self.chrome
                .pane_grid
                .panes()
                .iter()
                .find(|p| p.external_id == Some(id))
                .map(|p| p.rect)
        });
        let theme = *self.chrome.ide_theme();
        let term = self.pane_terminals.entry(id).or_insert_with(|| {
            let cols = rect
                .map(|r| ((r.w / cell_w).floor() as u32).max(2))
                .unwrap_or(80);
            let rows = rect
                .map(|r| ((r.h / cell_h).floor() as u32).max(2))
                .unwrap_or(24);
            let mut t = Terminal::new(cols, rows);
            seed_terminal_theme(&mut t, &theme);
            t
        });
        if !bytes.is_empty() {
            term.feed(bytes);
        }
        let effects: Vec<WasmEffect> = term
            .inner
            .drain_effects()
            .filter_map(WasmEffect::from_core)
            .collect();
        serde_wasm_bindgen::to_value(&effects).unwrap_or(JsValue::NULL)
    }

    /// True when a pane terminal already exists for `external_id` —
    /// TS uses this to decide whether to seed a fresh one from its
    /// replay buffer.
    pub fn pane_terminal_exists(&self, external_id: u32) -> bool {
        self.pane_terminals.contains_key(&(external_id as u64))
    }

    /// Apply daemon-authoritative cwd metadata to the terminal currently
    /// owning the composer. OSC 7 may not be present in replayed PTY bytes.
    pub fn set_active_terminal_cwd(&mut self, cwd: String) {
        let path = std::path::PathBuf::from(cwd);
        self.rendered.terminal.inner.current_directory = Some(path.clone());
        if self.chrome.pane_grid.is_split() {
            if let Some(id) = self.chrome.pane_grid.focused_external_id() {
                if let Some(terminal) = self.pane_terminals.get_mut(&id) {
                    terminal.inner.current_directory = Some(path);
                }
            }
        }
    }

    /// Drop one pane terminal (its pane closed or its session took
    /// over the main grid).
    pub fn remove_pane_terminal(&mut self, external_id: u32) {
        self.pane_terminals.remove(&(external_id as u64));
    }

    /// Retain only the pane terminals whose external ids appear in
    /// `keep_json` (a JSON array of ids).
    pub fn prune_pane_terminals(&mut self, keep_json: &str) -> Result<(), JsValue> {
        let keep: Vec<u64> = serde_json::from_str(keep_json)
            .map_err(|e| JsValue::from_str(&format!("pane prune parse: {e}")))?;
        self.pane_terminals.retain(|id, _| keep.contains(id));
        Ok(())
    }

    // ----------------------------------------------------------------
    // Per-pane tab strips + breadcrumbs (desktop pane_tabs parity).
    // ----------------------------------------------------------------

    /// Replace one pane's local tab strip. JSON:
    /// `[{title, path?, kind?}]`; an empty list drops the strip. The
    /// breadcrumbs row derives from the active tab's path.
    pub fn set_pane_tabs(
        &mut self,
        external_id: u32,
        tabs_json: &str,
        active: u32,
    ) -> Result<(), JsValue> {
        use neoism_ui::panels::buffer_tabs::BufferTab;
        #[derive(serde::Deserialize)]
        struct JsPaneTab {
            title: String,
            #[serde(default)]
            path: Option<String>,
            #[serde(default)]
            kind: Option<String>,
        }
        let raw: Vec<JsPaneTab> = serde_json::from_str(tabs_json)
            .map_err(|e| JsValue::from_str(&format!("pane tabs parse: {e}")))?;
        let tabs: Vec<BufferTab<()>> = raw
            .into_iter()
            .map(|t| {
                let is_terminal = t.kind.as_deref() == Some("terminal");
                BufferTab {
                    title: t.title,
                    modified: false,
                    custom_icon: None,
                    // A pane-strip terminal tab is path-less (sticky —
                    // panes close through the pane grid, not the X).
                    path: (!is_terminal)
                        .then(|| t.path.as_deref().map(std::path::PathBuf::from))
                        .flatten(),
                    markdown: t
                        .path
                        .as_deref()
                        .map(|p| {
                            neoism_ui::syntax::Lang::from_path(p)
                                == neoism_ui::syntax::Lang::Markdown
                        })
                        .unwrap_or(false),
                    terminal_route_id: None,
                    neoism_agent_route_id: None,
                    chrome_page: None,
                    agent_kind: None,
                }
            })
            .collect();
        self.chrome
            .set_pane_tabs(external_id as u64, tabs, active as usize);
        Ok(())
    }

    /// Drop pane strips whose panes went away. `keep_json` is a JSON
    /// array of surviving pane external ids.
    pub fn retain_pane_tabs(&mut self, keep_json: &str) -> Result<(), JsValue> {
        let keep: Vec<u64> = serde_json::from_str(keep_json)
            .map_err(|e| JsValue::from_str(&format!("pane tabs prune parse: {e}")))?;
        self.chrome.retain_pane_tabs(&keep);
        Ok(())
    }

    /// Drain queued per-pane tab-strip interactions:
    /// `[{external_id, kind: "activate"|"close"|"new_tab", index}]`.
    pub fn drain_pane_tab_intents(&mut self) -> Result<JsValue, JsValue> {
        let intents = std::mem::take(&mut self.pending_pane_tab_intents);
        serde_wasm_bindgen::to_value(&intents)
            .map_err(|e| JsValue::from_str(&format!("pane tab intents: {e}")))
    }

    // ----------------------------------------------------------------
    // Workspace-strip tab drag — the SHARED begin_drag / update_drag /
    // end_drag pipeline (reorder inside the strip, tear-out below it).
    // ----------------------------------------------------------------

    /// Arm the shared strip drag at a window point. Returns the tab
    /// index the drag started on, or -1 when the point is not a tab
    /// body (close X / "+" never arm drags).
    pub fn buffer_tab_begin_drag(&mut self, x: f32, y: f32) -> i32 {
        let rect = self.chrome.layout().buffer_tabs;
        let Some(TabHit::Activate(ix)) = self
            .chrome
            .buffer_tabs
            .hit_test(x, y, rect.x, rect.y, rect.w)
        else {
            return -1;
        };
        self.chrome.buffer_tabs.begin_drag(ix, x, y, rect.x, rect.w);
        self.tab_drag_begin_ix = Some(ix);
        ix as i32
    }

    /// Advance the shared strip drag. Returns true when the drag's
    /// render state changed (slot swap, tear-out arming) — the host
    /// schedules a repaint.
    pub fn buffer_tab_update_drag(&mut self, x: f32, y: f32) -> bool {
        if self.tab_drag_begin_ix.is_none() {
            return false;
        }
        let rect = self.chrome.layout().buffer_tabs;
        self.chrome
            .buffer_tabs
            .update_drag(x, y, rect.x, rect.y, rect.w)
    }

    /// True while the armed drag has crossed the tear-out threshold
    /// below the strip (the release would tear the tab out of the
    /// strip instead of reordering).
    pub fn buffer_tab_drag_tear_armed(&self) -> bool {
        self.chrome
            .buffer_tabs
            .drag_state()
            .is_some_and(|drag| drag.active && drag.tear_out_armed)
    }

    /// Release the shared strip drag. Returns
    /// `{kind: "none"|"reorder"|"tear_out", from, to, index,
    ///   release: "markdown"|"file"|"agent"|"drop"}` — `from`/`to` are
    /// the reorder endpoints, `index`/`release` classify a tear-out
    /// through the shared `tab_drag_release_kind` policy.
    pub fn buffer_tab_end_drag(&mut self) -> Result<JsValue, JsValue> {
        use neoism_ui::panels::buffer_tabs::{
            tab_drag_release_kind, DragRelease, TabDragReleaseKind,
        };

        #[derive(serde::Serialize)]
        struct JsTabDragRelease {
            kind: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            from: Option<usize>,
            #[serde(skip_serializing_if = "Option::is_none")]
            to: Option<usize>,
            #[serde(skip_serializing_if = "Option::is_none")]
            index: Option<usize>,
            #[serde(skip_serializing_if = "Option::is_none")]
            release: Option<&'static str>,
        }

        let from = self.tab_drag_begin_ix.take();
        let to = self
            .chrome
            .buffer_tabs
            .drag_state()
            .map(|drag| drag.current_ix);
        let release = self.chrome.buffer_tabs.end_drag(false);
        let out = match release {
            DragRelease::None => JsTabDragRelease {
                kind: "none",
                from: None,
                to: None,
                index: None,
                release: None,
            },
            DragRelease::Reorder => JsTabDragRelease {
                kind: "reorder",
                from,
                to,
                index: None,
                release: None,
            },
            DragRelease::TearOut { ix, tab } => {
                let kind = tab_drag_release_kind(
                    tab.path.is_some(),
                    tab.markdown,
                    tab.agent_kind.is_some(),
                );
                JsTabDragRelease {
                    kind: "tear_out",
                    from,
                    to: None,
                    index: Some(ix),
                    release: Some(match kind {
                        TabDragReleaseKind::Markdown => "markdown",
                        TabDragReleaseKind::File => "file",
                        TabDragReleaseKind::Agent => "agent",
                        TabDragReleaseKind::Drop => "drop",
                    }),
                }
            }
            DragRelease::MoveOut { tab } => {
                // end_drag(false) never yields MoveOut, but keep the
                // arm honest for a future cross-strip destination.
                let kind = tab_drag_release_kind(
                    tab.path.is_some(),
                    tab.markdown,
                    tab.agent_kind.is_some(),
                );
                JsTabDragRelease {
                    kind: "tear_out",
                    from,
                    to: None,
                    index: from,
                    release: Some(match kind {
                        TabDragReleaseKind::Markdown => "markdown",
                        TabDragReleaseKind::File => "file",
                        TabDragReleaseKind::Agent => "agent",
                        TabDragReleaseKind::Drop => "drop",
                    }),
                }
            }
        };
        serde_wasm_bindgen::to_value(&out)
            .map_err(|e| JsValue::from_str(&format!("tab drag release: {e}")))
    }

    /// Abort the shared strip drag without releasing (pointer cancel).
    pub fn buffer_tab_cancel_drag(&mut self) {
        self.tab_drag_begin_ix = None;
        let _ = self.chrome.buffer_tabs.end_drag(false);
    }

    /// Shared drag-to-split drop hit test for the web tab→pane drag.
    ///
    /// `panes_json` is the host's current normalized pane list (the
    /// `panes` array `apply_session_layout_policy` /
    /// `mirror_pane_layout_snapshot` returned: `[{external_id, x, y,
    /// w, h}]` in unit space) and `(x, y)` is the pointer in that
    /// same unit space. The hit test runs through the SAME shared
    /// geometry the desktop PaneGrid uses
    /// (`session_layout::geometry::drop_zone_at` with
    /// `pane_grid::DEFAULT_EDGE_FRAC`), so the edge-band fraction and
    /// the half-split preview can never drift from the shared
    /// constants again. Edge bands and highlight rects are fractions
    /// of each pane's own extent, so running the test in unit space
    /// yields the identical placement the desktop gets in pixels.
    ///
    /// Returns `null` when the point misses every pane, else
    /// `{external_id, placement: "left"|"right"|"top"|"bottom"|"center",
    ///   rect: {x, y, w, h}}` where `rect` is the normalized preview
    /// highlight (the region the dragged tab would occupy if
    /// released now).
    pub fn pane_drop_target(
        &self,
        panes_json: &str,
        x: f32,
        y: f32,
    ) -> Result<JsValue, JsValue> {
        use neoism_ui::layout::Rect;
        use neoism_ui::panels::pane_grid::DEFAULT_EDGE_FRAC;
        use neoism_ui::session_layout::geometry::{
            drop_zone_at, DropPlacement, PaneRect, SolvedLayout,
        };
        use neoism_ui::session_layout::tree::SessionTreeLeafId;

        #[derive(serde::Deserialize)]
        struct JsPane {
            external_id: u64,
            x: f32,
            y: f32,
            w: f32,
            h: f32,
        }
        #[derive(serde::Serialize)]
        struct JsDropRect {
            x: f32,
            y: f32,
            w: f32,
            h: f32,
        }
        #[derive(serde::Serialize)]
        struct JsDropTarget {
            external_id: u64,
            placement: &'static str,
            rect: JsDropRect,
        }

        let raw: Vec<JsPane> = serde_json::from_str(panes_json)
            .map_err(|e| JsValue::from_str(&format!("panes parse: {e}")))?;
        // Synthesize a SolvedLayout whose leaf ids are the pane
        // indices, so the shared hit test runs against the host's
        // already-solved rects without rebuilding a SessionTree.
        let solved = SolvedLayout {
            panes: raw
                .iter()
                .enumerate()
                .map(|(ix, p)| PaneRect {
                    leaf: SessionTreeLeafId(ix as u64),
                    external_id: Some(p.external_id),
                    kind: SessionLeafKind::Editor,
                    rect: Rect::new(p.x, p.y, p.w, p.h),
                    focused: false,
                    path: Vec::new(),
                })
                .collect(),
            dividers: Vec::new(),
        };
        let Some(zone) = drop_zone_at(&solved, x, y, DEFAULT_EDGE_FRAC) else {
            return Ok(JsValue::NULL);
        };
        let external_id = raw
            .get(zone.target.0 as usize)
            .map(|p| p.external_id)
            .ok_or_else(|| JsValue::from_str("drop target out of range"))?;
        let placement = match zone.placement {
            DropPlacement::Left => "left",
            DropPlacement::Right => "right",
            DropPlacement::Top => "top",
            DropPlacement::Bottom => "bottom",
            DropPlacement::Center => "center",
        };
        serde_wasm_bindgen::to_value(&JsDropTarget {
            external_id,
            placement,
            rect: JsDropRect {
                x: zone.highlight.x,
                y: zone.highlight.y,
                w: zone.highlight.w,
                h: zone.highlight.h,
            },
        })
        .map_err(|e| JsValue::from_str(&format!("drop target: {e}")))
    }

    /// JS calls this to flag which tab the user wants visible.
    /// Index 0 is always the Terminal tab — selecting it shows
    /// the cell grid + splash. Any other index switches to the
    /// file-viewer pane and the `tab_content` for that index is
    /// drawn over the terminal rect.
    pub fn set_active_tab(&mut self, idx: u32) {
        self.sync_active_tab_state(idx as usize);
        self.sync_status_mode_for_active_tab_index();
        self.relayout_chrome();
    }

    /// JS pushes the (possibly long) text content for a tab here
    /// after fetching it from the daemon via FilesService. `path`
    /// is the original file path the content came from — used to
    /// derive the source language for syntax highlighting.
    pub fn set_tab_content(&mut self, idx: u32, text: &str, path: &str) {
        let key = idx as usize;
        self.tab_contents.insert(key, text.to_string());
        self.tab_paths.insert(key, path.to_string());
        // If the host is currently viewing this tab, refresh the
        // chrome's cached content + lang so the next frame paints
        // it with the right token colors.
        if self.active_tab_index == key {
            self.sync_active_tab_state(key);
        } else if neoism_ui::syntax::Lang::from_path(path)
            == neoism_ui::syntax::Lang::Markdown
        {
            // Host tab indices can drift from ours (sticky terminal
            // slot). A markdown pane left contentless renders BLACK,
            // so feed the pane directly whenever the content's path
            // is a .md — worst case we refresh a background doc.
            self.chrome
                .set_markdown_content(Some(text.to_string()), Some(path));
        }
    }
}

impl ChromeBridge {
    /// Re-seed the chrome's shared [`PaneGrid`] with the freshly
    /// mutated session tree and re-solve it against the current
    /// content rect. Called after every applied layout policy /
    /// snapshot mirror so pointer interactions (divider drag, drop
    /// zones, focus-by-click) and the chrome's pane-aware painters
    /// always run against the tree the host is about to render.
    pub(crate) fn adopt_pane_tree(
        &mut self,
        tree: neoism_ui::session_layout::tree::SessionTree,
    ) {
        self.chrome.pane_grid = PaneGrid::from_tree(tree);
        // `Chrome::set_layout` re-solves the grid against the terminal
        // rect (see shared/src/chrome/events.rs).
        self.relayout_chrome();
    }

    /// While the pane grid is split, paint every visible terminal pane
    /// from its own pane-sized terminal in `pane_terminals` and
    /// publish the painted ids through `Chrome::set_host_drawn_panes`
    /// (so the chrome's unfocused-pane pass leaves them alone).
    /// Returns true when the split pipeline owned the frame — the
    /// caller's full-rect terminal pipeline must stand down.
    pub(crate) fn draw_split_terminal_panes(&mut self) -> bool {
        if !self.chrome.pane_grid.is_split() || self.chrome.is_neoism_agent_tab_active() {
            self.chrome.set_host_drawn_panes(Vec::new());
            return false;
        }
        let cell_w = self.rendered.cell_w.max(1.0);
        let cell_h = self.rendered.cell_h.max(1.0);
        let panes: Vec<(u64, neoism_ui::layout::Rect, bool)> = self
            .chrome
            .pane_grid
            .panes()
            .iter()
            .filter_map(|p| p.external_id.map(|id| (id, p.rect, p.focused)))
            .collect();
        // The raw-cells pointer mapping belongs to the (suppressed)
        // full-rect grid; clear it so stale composed-window sources
        // can't anchor clicks.
        self.rendered.pointer.frame_sources = None;
        let focused_hosts_editor = !self.chrome.is_terminal_tab_active();
        let mut drawn = Vec::new();
        for (id, rect, focused) in panes {
            // The focused pane hosts the active EDITOR surface when a
            // non-terminal tab is active — its (stale) pane terminal
            // must not paint under/over the editor content.
            if focused && focused_hosts_editor {
                continue;
            }
            // Cells paint into the pane's CONTENT rect — below the
            // pane's local tab strip / breadcrumbs when present.
            let rect = self.chrome.pane_content_rect(id).unwrap_or(rect);
            let Some(term) = self.pane_terminals.get_mut(&id) else {
                continue;
            };
            let want_cols = ((rect.w / cell_w).floor() as u32).max(2);
            let want_rows = ((rect.h / cell_h).floor() as u32).max(2);
            if term.inner.columns() as u32 != want_cols
                || term.inner.screen_lines() as u32 != want_rows
            {
                term.resize(want_cols, want_rows);
            }
            let term = &*term;
            self.rendered.draw_pane_terminal_cells_in(
                term,
                rect.x,
                rect.y,
                [rect.x, rect.y, rect.w, rect.h],
                // A focused strip, reached via Alt+Up, owns the only cursor.
                // Keep split-pane terminals from retaining their block caret
                // while the shared tab trail paints the desktop-style bar.
                focused && !self.chrome.buffer_tabs.is_focused(),
            );
            drawn.push(id);
        }
        self.chrome.set_host_drawn_panes(drawn);
        true
    }
}
