use super::*;
use neoism_ui::panels::buffer_tabs::{
    apply_buffer_tab_policy as shared_apply_buffer_tab_policy, BufferTabPolicyInput,
    BufferTabPolicyOperation,
};
use neoism_ui::session_layout::{
    SessionLayout, SessionLeafKind, SessionLeafSpec, SessionNode, SplitAxis,
    SplitPlacement,
};

#[wasm_bindgen]
impl ChromeBridge {
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
                BufferTab {
                    title: t.title,
                    modified: false,
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
                    chrome_page: None,
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
                let is_terminal = tab.kind.as_deref() == Some("terminal")
                    || (tab.path.is_none() && !is_agent);
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

    /// Apply shared SessionLayout policy for JS-owned visible pane state.
    ///
    /// Web still owns DOM/PTY/nvim side effects. This bridge keeps the
    /// split/focus/close/resize tree itself in the same shared Rust model
    /// that desktop now mirrors for native panes, then returns computed
    /// normalized rectangles for the browser overlay/dispatcher.
    pub fn apply_session_layout_policy(
        &self,
        state_json: Option<String>,
        operation: &str,
        axis: Option<String>,
        title: Option<String>,
        external_id: Option<u32>,
    ) -> Result<JsValue, JsValue> {
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

        fn push_pane_rects(
            layout: &SessionLayout,
            node_id: neoism_ui::session_layout::SessionNodeId,
            rect: (f32, f32, f32, f32),
            out: &mut Vec<WebPaneRect>,
        ) -> Result<(), JsValue> {
            match layout
                .node(node_id)
                .ok_or_else(|| JsValue::from_str("layout references a missing node"))?
            {
                SessionNode::Leaf(leaf) => {
                    let Some(external_id) = leaf.external_id else {
                        return Ok(());
                    };
                    let kind = match &leaf.kind {
                        SessionLeafKind::Terminal => "terminal".to_string(),
                        SessionLeafKind::Editor => "editor".to_string(),
                        SessionLeafKind::Agent => "agent".to_string(),
                        SessionLeafKind::Custom(kind) => kind.clone(),
                    };
                    out.push(WebPaneRect {
                        external_id,
                        leaf_id: leaf.id.0,
                        kind,
                        title: leaf.title.clone(),
                        focused: layout.focused_leaf() == leaf.id,
                        x: rect.0,
                        y: rect.1,
                        w: rect.2,
                        h: rect.3,
                    });
                }
                SessionNode::Split(split) => {
                    let (x, y, w, h) = rect;
                    match split.axis {
                        // Mirrors desktop's split-intent mapping:
                        // Horizontal splits produce left/right panes.
                        SplitAxis::Horizontal => {
                            let first_w = w * split.ratio;
                            push_pane_rects(
                                layout,
                                split.first,
                                (x, y, first_w, h),
                                out,
                            )?;
                            push_pane_rects(
                                layout,
                                split.second,
                                (x + first_w, y, w - first_w, h),
                                out,
                            )?;
                        }
                        // Vertical splits produce top/bottom panes.
                        SplitAxis::Vertical => {
                            let first_h = h * split.ratio;
                            push_pane_rects(
                                layout,
                                split.first,
                                (x, y, w, first_h),
                                out,
                            )?;
                            push_pane_rects(
                                layout,
                                split.second,
                                (x, y + first_h, w, h - first_h),
                                out,
                            )?;
                        }
                    }
                }
            }
            Ok(())
        }

        let mut layout = if let Some(json) = state_json.filter(|json| !json.is_empty()) {
            serde_json::from_str::<SessionLayout>(&json)
                .map_err(|e| JsValue::from_str(&format!("layout parse: {e}")))?
        } else {
            let initial_kind = if operation == "init_terminal" {
                "terminal"
            } else {
                "editor"
            };
            SessionLayout::new(spec_for(
                Some(initial_kind),
                title.clone().or_else(|| Some("Editor 1".to_string())),
                Some(external_id.unwrap_or(1)),
            ))
        };

        let before = serde_json::to_string(&layout)
            .map_err(|e| JsValue::from_str(&format!("layout snapshot: {e}")))?;

        match operation {
            "init" | "init_editor" | "init_terminal" => {}
            "split" => {
                layout
                    .split_focused(
                        split_axis(axis.as_deref()),
                        SplitPlacement::After,
                        spec_for(Some("editor"), title, external_id),
                    )
                    .map_err(|e| JsValue::from_str(&format!("layout split: {e:?}")))?;
            }
            "focus_next" => {
                layout
                    .focus_adjacent_leaf(false, true)
                    .map_err(|e| JsValue::from_str(&format!("layout focus: {e:?}")))?;
            }
            "focus_prev" => {
                layout
                    .focus_adjacent_leaf(true, true)
                    .map_err(|e| JsValue::from_str(&format!("layout focus: {e:?}")))?;
            }
            "focus_external" => {
                let external_id = external_id.ok_or_else(|| {
                    JsValue::from_str("layout focus_external requires external_id")
                })? as u64;
                let leaf = layout
                    .active_leaves()
                    .into_iter()
                    .find(|leaf_id| {
                        layout.leaf(*leaf_id).and_then(|leaf| leaf.external_id)
                            == Some(external_id)
                    })
                    .ok_or_else(|| {
                        JsValue::from_str(&format!(
                            "layout focus_external missing pane {external_id}"
                        ))
                    })?;
                layout.focus_leaf(leaf).map_err(|e| {
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
                let already = layout.active_leaves().into_iter().any(|leaf_id| {
                    layout.leaf(leaf_id).and_then(|leaf| leaf.external_id)
                        == Some(external_id)
                });
                if !already {
                    layout
                        .split_focused(
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
                layout
                    .close_focused_leaf()
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
                layout
                    .resize_split_toward_leaf(layout.focused_leaf(), Some(axis), delta)
                    .map_err(|e| JsValue::from_str(&format!("layout resize: {e:?}")))?;
            }
            other => {
                return Err(JsValue::from_str(&format!(
                    "unknown session-layout policy operation: {other}"
                )));
            }
        }
        layout
            .validate()
            .map_err(|e| JsValue::from_str(&format!("layout validate: {e:?}")))?;

        let mut panes = Vec::new();
        push_pane_rects(
            &layout,
            layout.active_tab().root,
            (0.0, 0.0, 1.0, 1.0),
            &mut panes,
        )?;
        let state_json = serde_json::to_string(&layout)
            .map_err(|e| JsValue::from_str(&format!("layout serialize: {e}")))?;
        let result = WebSessionLayoutPolicyResult {
            focused_external_id: layout.focused_external_id(),
            active_external_ids: layout.active_leaf_external_ids(),
            changed: before != state_json,
            state_json,
            panes,
        };
        serde_wasm_bindgen::to_value(&result)
            .map_err(|e| JsValue::from_str(&format!("layout result: {e}")))
    }

    /// Mirror a daemon `PaneLayoutSnapshot` (the authoritative pane
    /// tree the desktop renders) into the same `SessionLayout`-derived
    /// pane rectangles the local `apply_session_layout_policy` path
    /// produces.
    ///
    /// The web previously fed the daemon's snapshot JSON straight into
    /// `apply_session_layout_policy`, but the wire snapshot
    /// (`{schema_version, root: {kind, axis, ratios, children}}`) is a
    /// different serde shape than the policy's `SessionLayout`
    /// (`{tabs, nodes, ...}`), so the parse failed and the web silently
    /// kept its stale, locally-derived split layout. Lowering the
    /// snapshot through the shared `SessionLayout::from_pane_layout_snapshot`
    /// converter makes the web render the exact split intent — axis,
    /// ratios, nesting, focus — the desktop mirrors.
    pub fn mirror_pane_layout_snapshot(
        &self,
        snapshot_json: &str,
    ) -> Result<JsValue, JsValue> {
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

        fn push_pane_rects(
            layout: &SessionLayout,
            node_id: neoism_ui::session_layout::SessionNodeId,
            rect: (f32, f32, f32, f32),
            out: &mut Vec<WebPaneRect>,
        ) -> Result<(), JsValue> {
            match layout
                .node(node_id)
                .ok_or_else(|| JsValue::from_str("layout references a missing node"))?
            {
                SessionNode::Leaf(leaf) => {
                    let Some(external_id) = leaf.external_id else {
                        return Ok(());
                    };
                    let kind = match &leaf.kind {
                        SessionLeafKind::Terminal => "terminal".to_string(),
                        SessionLeafKind::Editor => "editor".to_string(),
                        SessionLeafKind::Agent => "agent".to_string(),
                        SessionLeafKind::Custom(kind) => kind.clone(),
                    };
                    out.push(WebPaneRect {
                        external_id,
                        leaf_id: leaf.id.0,
                        kind,
                        title: leaf.title.clone(),
                        focused: layout.focused_leaf() == leaf.id,
                        x: rect.0,
                        y: rect.1,
                        w: rect.2,
                        h: rect.3,
                    });
                }
                SessionNode::Split(split) => {
                    let (x, y, w, h) = rect;
                    match split.axis {
                        SplitAxis::Horizontal => {
                            let first_w = w * split.ratio;
                            push_pane_rects(
                                layout,
                                split.first,
                                (x, y, first_w, h),
                                out,
                            )?;
                            push_pane_rects(
                                layout,
                                split.second,
                                (x + first_w, y, w - first_w, h),
                                out,
                            )?;
                        }
                        SplitAxis::Vertical => {
                            let first_h = h * split.ratio;
                            push_pane_rects(
                                layout,
                                split.first,
                                (x, y, w, first_h),
                                out,
                            )?;
                            push_pane_rects(
                                layout,
                                split.second,
                                (x, y + first_h, w, h - first_h),
                                out,
                            )?;
                        }
                    }
                }
            }
            Ok(())
        }

        let snapshot = serde_json::from_str::<
            neoism_protocol::workspace::PaneLayoutSnapshot,
        >(snapshot_json)
        .map_err(|e| JsValue::from_str(&format!("snapshot parse: {e}")))?;
        let layout = SessionLayout::from_pane_layout_snapshot(&snapshot)
            .map_err(|e| JsValue::from_str(&format!("snapshot mirror: {e:?}")))?;

        let mut panes = Vec::new();
        push_pane_rects(
            &layout,
            layout.active_tab().root,
            (0.0, 0.0, 1.0, 1.0),
            &mut panes,
        )?;
        let state_json = serde_json::to_string(&layout)
            .map_err(|e| JsValue::from_str(&format!("layout serialize: {e}")))?;
        let result = WebSessionLayoutPolicyResult {
            focused_external_id: layout.focused_external_id(),
            active_external_ids: layout.active_leaf_external_ids(),
            changed: true,
            state_json,
            panes,
        };
        serde_wasm_bindgen::to_value(&result)
            .map_err(|e| JsValue::from_str(&format!("layout result: {e}")))
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
