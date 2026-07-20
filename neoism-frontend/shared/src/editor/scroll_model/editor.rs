#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorViewport {
    pub line_count: usize,
    /// Zero-indexed first visible line.
    pub topline: usize,
    /// Zero-indexed exclusive bottom line from nvim's viewport event.
    pub botline: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorScrollbarModel {
    pub display_offset: usize,
    pub history_size: usize,
    pub screen_lines: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EditorWheelAction {
    Idle,
    SendRows {
        direction: EditorScrollDirection,
        rows: u32,
    },
    EdgeElastic {
        clear_top_snapshots: bool,
        clear_bottom_snapshots: bool,
        elastic_pixels: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorScrollbarDragTarget {
    pub topline: usize,
    pub nvim_topline: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarClickBand {
    /// Pane rectangle in physical pixels: `[left, top, width, height]`.
    pub panel_rect: [f32; 4],
    pub scale_factor: f32,
    pub hit_width_logical_px: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarPaneKind {
    Editor,
    Terminal,
    Markdown,
    Agent,
    Tags,
}

impl ScrollbarPaneKind {
    pub fn owns_global_scrollbar_band(self) -> bool {
        matches!(self, Self::Editor | Self::Terminal)
    }

    pub fn can_show_global_scrollbar(self) -> bool {
        matches!(self, Self::Editor | Self::Terminal)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarClickIntent {
    Ignore,
    SwallowEmptyBand,
    StartDrag { jump_to_track: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollbarClickContext {
    pub pane_kind: ScrollbarPaneKind,
    pub band_contains_pointer: bool,
    pub has_scroll_state: bool,
    pub hit_scrollbar_geometry: bool,
    pub grabbed_thumb: bool,
}

impl ScrollbarClickContext {
    pub fn intent(self) -> ScrollbarClickIntent {
        if !self.has_scroll_state {
            if self.pane_kind.owns_global_scrollbar_band() && self.band_contains_pointer {
                return ScrollbarClickIntent::SwallowEmptyBand;
            }
            return ScrollbarClickIntent::Ignore;
        }

        if !self.hit_scrollbar_geometry {
            return ScrollbarClickIntent::Ignore;
        }

        ScrollbarClickIntent::StartDrag {
            jump_to_track: !self.grabbed_thumb,
        }
    }
}

impl ScrollbarClickBand {
    pub fn contains_logical_point(self, mouse_x: f32, mouse_y: f32) -> bool {
        if self.scale_factor <= 0.0 || !self.scale_factor.is_finite() {
            return false;
        }

        let pane_left = self.panel_rect[0] / self.scale_factor;
        let pane_right = (self.panel_rect[0] + self.panel_rect[2]) / self.scale_factor;
        let pane_top = self.panel_rect[1] / self.scale_factor;
        let pane_bottom = (self.panel_rect[1] + self.panel_rect[3]) / self.scale_factor;
        let hit_width = self.hit_width_logical_px.max(0.0);
        let bar_left = pane_right - hit_width;

        mouse_x >= bar_left.min(pane_right)
            && mouse_x <= pane_right
            && mouse_x >= pane_left
            && mouse_y >= pane_top
            && mouse_y <= pane_bottom
    }
}

pub fn active_scrollbar_drag_rich_text_id(
    drag_state_rich_text_id: Option<usize>,
    panel_state_rich_text_id: Option<usize>,
    current_rich_text_id: usize,
) -> usize {
    drag_state_rich_text_id
        .or(panel_state_rich_text_id)
        .unwrap_or(current_rich_text_id)
}

pub fn display_offset_delta(current: usize, new_offset: usize) -> Option<i32> {
    let delta = new_offset as i128 - current as i128;
    (delta != 0).then_some(delta.clamp(i32::MIN as i128, i32::MAX as i128) as i32)
}

impl EditorViewport {
    pub fn visible_lines(self) -> usize {
        self.botline.saturating_sub(self.topline).max(1)
    }

    pub fn max_topline(self) -> Option<usize> {
        let visible = self.visible_lines();
        (self.line_count > visible).then_some(self.line_count - visible)
    }

    pub fn scrollbar_model(self) -> Option<EditorScrollbarModel> {
        let visible = self.visible_lines();
        if self.line_count <= visible {
            return None;
        }
        Some(EditorScrollbarModel {
            display_offset: self.line_count.saturating_sub(self.botline),
            history_size: self.line_count - visible,
            screen_lines: visible,
        })
    }

    /// Convert terminal-style scrollbar offset back into a zero-indexed
    /// nvim topline. Offset 0 means bottom of file.
    pub fn topline_for_display_offset(self, display_offset: usize) -> Option<usize> {
        let max_topline = self.max_topline()?;
        Some(max_topline.saturating_sub(display_offset).min(max_topline))
    }

    pub fn scrollbar_drag_target(
        self,
        display_offset: usize,
    ) -> Option<EditorScrollbarDragTarget> {
        let topline = self.topline_for_display_offset(display_offset)?;
        Some(EditorScrollbarDragTarget {
            topline,
            nvim_topline: topline + 1,
        })
    }

    pub fn at_top(self) -> bool {
        self.line_count > 0 && self.topline == 0
    }

    pub fn at_bottom(self) -> bool {
        self.line_count > 0 && self.botline >= self.line_count
    }

    pub fn wheel_raw_action(self, delta_pixels: f32) -> EditorWheelAction {
        if delta_pixels == 0.0 || !delta_pixels.is_finite() {
            return EditorWheelAction::Idle;
        }
        if delta_pixels > 0.0 && self.at_top() {
            EditorWheelAction::EdgeElastic {
                clear_top_snapshots: true,
                clear_bottom_snapshots: false,
                elastic_pixels: delta_pixels,
            }
        } else if delta_pixels < 0.0 && self.at_bottom() {
            EditorWheelAction::EdgeElastic {
                clear_top_snapshots: false,
                clear_bottom_snapshots: true,
                elastic_pixels: delta_pixels,
            }
        } else {
            EditorWheelAction::Idle
        }
    }

    pub fn wheel_commit_action(
        self,
        committed_rows: i32,
        cell_height: f32,
    ) -> EditorWheelAction {
        if committed_rows == 0 {
            return EditorWheelAction::Idle;
        }

        let direction = if committed_rows > 0 {
            EditorScrollDirection::Up
        } else {
            EditorScrollDirection::Down
        };

        if direction == EditorScrollDirection::Up && self.at_top() {
            EditorWheelAction::EdgeElastic {
                clear_top_snapshots: true,
                clear_bottom_snapshots: false,
                elastic_pixels: committed_rows as f32 * cell_height.max(1.0),
            }
        } else if direction == EditorScrollDirection::Down && self.at_bottom() {
            EditorWheelAction::EdgeElastic {
                clear_top_snapshots: false,
                clear_bottom_snapshots: true,
                elastic_pixels: committed_rows as f32 * cell_height.max(1.0),
            }
        } else {
            EditorWheelAction::SendRows {
                direction,
                rows: committed_rows.unsigned_abs(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Ex command intercept plans (Wave 13-C)
// ---------------------------------------------------------------------------

/// Pure parse of an editor ex command (`:foo bar`) into a (head, tail)
/// pair. Trims the leading `:`, splits on whitespace, lowercases the
/// head. Returns `None` for empty commands.
pub fn parse_ex_command(cmd: &str) -> Option<(String, String)> {
    let trimmed = cmd.trim().trim_start_matches(':').trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("").to_ascii_lowercase();
    let tail = parts.next().unwrap_or("").trim().to_string();
    Some((head, tail))
}

/// Classification of an ex command head for the *markdown* pane. The
/// host owns the actual jump / save / close side effects; this is a
/// pure decision so desktop + web can match exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownExCommandPlan {
    /// `$` → jump to last line.
    JumpToLastLine,
    /// `<NN>` → jump to a 1-indexed line (saturating to 1 if 0).
    JumpToLine(usize),
    /// `:w` / `:write` / `:w!` → save the buffer.
    Save,
    /// `:wq` / `:x` / `:exit` → save then close the focused buffer tab.
    SaveAndCloseFocusedBuffer,
    /// Notebook pane: run the selected code cell.
    RunNotebookCell,
    /// Notebook pane: run selected code cell and every code cell below it.
    RunNotebookCellAndBelow,
    /// Notebook pane: queue every code cell for execution.
    RunAllNotebookCells,
    /// Notebook pane: insert a code cell above the selected cell.
    InsertNotebookCodeCellAbove,
    /// Notebook pane: insert a code cell below the selected cell.
    InsertNotebookCodeCellBelow,
    /// Notebook pane: insert a markdown cell above the selected cell.
    InsertNotebookMarkdownCellAbove,
    /// Notebook pane: insert a markdown cell below the selected cell.
    InsertNotebookMarkdownCellBelow,
    /// Notebook pane: delete the selected cell.
    DeleteNotebookCell,
    /// Notebook pane: move the selected cell up.
    MoveNotebookCellUp,
    /// Notebook pane: move the selected cell down.
    MoveNotebookCellDown,
    /// Notebook pane: interrupt the currently running kernel execution.
    InterruptNotebookKernel,
    /// Notebook pane: clear outputs and execution count for the selected cell.
    ClearNotebookCellOutput,
    /// Notebook pane: clear outputs and execution counts.
    ClearNotebookOutputs,
    /// Notebook pane: restart the kernel before the next run.
    RestartNotebookKernel,
    /// Not a markdown-specific intercept; fall through to the global
    /// ex-command table.
    PassThrough,
}

impl MarkdownExCommandPlan {
    pub fn classify(head: &str) -> Self {
        if head == "$" {
            return Self::JumpToLastLine;
        }
        if let Ok(line) = head.parse::<usize>() {
            return Self::JumpToLine(line.max(1));
        }
        match head {
            "w" | "w!" | "write" | "write!" => Self::Save,
            "wq" | "wq!" | "x" | "x!" | "exit" => Self::SaveAndCloseFocusedBuffer,
            "runcell" | "run-cell" | "notebookruncell" | "notebook-run-cell" => {
                Self::RunNotebookCell
            }
            "runbelow"
            | "run-below"
            | "runcellandbelow"
            | "run-cell-and-below"
            | "notebookrunbelow"
            | "notebook-run-below"
            | "notebookruncellandbelow"
            | "notebook-run-cell-and-below" => Self::RunNotebookCellAndBelow,
            "runall" | "run-all" | "notebookrunall" | "notebook-run-all" => {
                Self::RunAllNotebookCells
            }
            "insertcodeabove"
            | "insert-code-above"
            | "addcodeabove"
            | "add-code-above"
            | "notebookinsertcodeabove"
            | "notebook-insert-code-above" => Self::InsertNotebookCodeCellAbove,
            "insertcode"
            | "insert-code"
            | "insertcodebelow"
            | "insert-code-below"
            | "addcode"
            | "add-code"
            | "addcodebelow"
            | "add-code-below"
            | "notebookinsertcode"
            | "notebook-insert-code"
            | "notebookinsertcodebelow"
            | "notebook-insert-code-below" => Self::InsertNotebookCodeCellBelow,
            "insertmarkdownabove"
            | "insert-markdown-above"
            | "addmarkdownabove"
            | "add-markdown-above"
            | "notebookinsertmarkdownabove"
            | "notebook-insert-markdown-above" => Self::InsertNotebookMarkdownCellAbove,
            "insertmarkdown"
            | "insert-markdown"
            | "insertmarkdownbelow"
            | "insert-markdown-below"
            | "addmarkdown"
            | "add-markdown"
            | "addmarkdownbelow"
            | "add-markdown-below"
            | "notebookinsertmarkdown"
            | "notebook-insert-markdown"
            | "notebookinsertmarkdownbelow"
            | "notebook-insert-markdown-below" => Self::InsertNotebookMarkdownCellBelow,
            "deletecell"
            | "delete-cell"
            | "delcell"
            | "del-cell"
            | "removecell"
            | "remove-cell"
            | "notebookdeletecell"
            | "notebook-delete-cell" => Self::DeleteNotebookCell,
            "movecellup"
            | "move-cell-up"
            | "cellup"
            | "cell-up"
            | "notebookmovecellup"
            | "notebook-move-cell-up" => Self::MoveNotebookCellUp,
            "movecelldown"
            | "move-cell-down"
            | "celldown"
            | "cell-down"
            | "notebookmovecelldown"
            | "notebook-move-cell-down" => Self::MoveNotebookCellDown,
            "interrupt"
            | "interruptkernel"
            | "interrupt-kernel"
            | "notebookinterrupt"
            | "notebook-interrupt"
            | "notebookinterruptkernel"
            | "notebook-interrupt-kernel" => Self::InterruptNotebookKernel,
            "clearoutputs"
            | "clear-outputs"
            | "notebookclearoutputs"
            | "notebook-clear-outputs" => Self::ClearNotebookOutputs,
            "clearoutput"
            | "clear-output"
            | "clearcelloutput"
            | "clear-cell-output"
            | "notebookclearoutput"
            | "notebook-clear-output"
            | "notebookclearcelloutput"
            | "notebook-clear-cell-output" => Self::ClearNotebookCellOutput,
            "restartkernel"
            | "restart-kernel"
            | "notebookrestartkernel"
            | "notebook-restart-kernel" => Self::RestartNotebookKernel,
            _ => Self::PassThrough,
        }
    }
}

/// Classification of an ex command head for the *global* (editor /
/// terminal) intercept table. Variants map 1:1 to the side-effect
/// functions on `Screen`. Tail-bearing commands carry the parsed tail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalExCommandPlan {
    Shaders,
    ThemePicker,
    ApplyTheme(String),
    OpenBuffersPicker,
    OpenFinderFiles,
    OpenFinderGrep,
    OpenFileTree,
    SetMinimap(Option<bool>),
    ToggleMinimap,
    CreateWorkspaceTerminalTab,
    /// Launch a workspace terminal for the given agent kind (Claude,
    /// Codex, OpenCode). The host owns the AgentKind enum, so we pass
    /// a string tag the host maps in.
    LaunchAgentTerminal {
        agent: AgentTag,
        tail: String,
    },
    /// `:opencode-acp` — start the native ACP agent if available, else
    /// fall back to a workspace terminal.
    StartOpenCodeAcp {
        tail: String,
    },
    SplitDown,
    SplitRight,
    OpenEmptyBufferTab,
    OpenPathInEditor(String),
    /// `:q` / `:quit` / `:close` — close the focused buffer tab.
    CloseFocusedBufferTab,
    /// `:wq` / `:x` / `:exit` — write the preferred editor route then
    /// close the focused buffer tab.
    WriteAndCloseFocusedBuffer,
    /// `:qa` / `:qall` — close every file tab in the focused pane (or
    /// workspace-wide if no split is focused).
    CloseAllBuffersInFocusedPaneOrWorkspace,
    /// `:wqa` / `:xa` — write all buffers (`wall`) then close every
    /// file tab.
    WriteAllAndCloseAllBuffers,
    /// No global intercept — let nvim handle it.
    PassThrough,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTag {
    Claude,
    Codex,
    OpenCode,
}

impl GlobalExCommandPlan {
    pub fn classify(head: &str, tail: &str) -> Self {
        let tail_lc = tail.to_ascii_lowercase();
        match head {
            "shaders" | "shaderpicker" => Self::Shaders,
            "shader" if tail.eq_ignore_ascii_case("picker") => Self::Shaders,
            "themepicker" | "theme-picker" => Self::ThemePicker,
            "theme" if tail.is_empty() || tail.eq_ignore_ascii_case("picker") => {
                Self::ThemePicker
            }
            "theme" => Self::ApplyTheme(tail.to_string()),
            "buffers" | "buffers!" | "ls" | "files" => Self::OpenBuffersPicker,
            "search"
                if tail.eq_ignore_ascii_case("files")
                    || tail.eq_ignore_ascii_case("file") =>
            {
                Self::OpenFinderFiles
            }
            "search"
                if tail.eq_ignore_ascii_case("words")
                    || tail.eq_ignore_ascii_case("word") =>
            {
                Self::OpenFinderGrep
            }
            "searchfiles" | "search-files" | "findfiles" | "find-files" | "ff" => {
                Self::OpenFinderFiles
            }
            "searchwords" | "search-words" | "findwords" | "find-words" | "grep"
            | "rg" | "fw" => Self::OpenFinderGrep,
            "tree" | "filetree" | "explorer" => Self::OpenFileTree,
            "minimap" | "mini-map" | "map" => Self::SetMinimap(match tail_lc.as_str() {
                "on" | "show" | "enable" | "enabled" => Some(true),
                "off" | "hide" | "disable" | "disabled" => Some(false),
                _ => None,
            }),
            "toggleminimap" | "toggle-minimap" => Self::ToggleMinimap,
            "terminal" | "term" | "te" => Self::CreateWorkspaceTerminalTab,
            "claude" => Self::LaunchAgentTerminal {
                agent: AgentTag::Claude,
                tail: tail.to_string(),
            },
            "codex" => Self::LaunchAgentTerminal {
                agent: AgentTag::Codex,
                tail: tail.to_string(),
            },
            "opencode" | "open-code" | "open_code" => Self::LaunchAgentTerminal {
                agent: AgentTag::OpenCode,
                tail: tail.to_string(),
            },
            "opencode-acp" | "open-code-acp" | "open_code_acp" => {
                Self::StartOpenCodeAcp {
                    tail: tail.to_string(),
                }
            }
            "opencode-terminal" | "open-code-terminal" | "open_code_terminal" => {
                Self::LaunchAgentTerminal {
                    agent: AgentTag::OpenCode,
                    tail: tail.to_string(),
                }
            }
            "split" | "sp" => Self::SplitDown,
            "vsplit" | "vsp" | "vert" | "vnew" => Self::SplitRight,
            "enew" | "new" | "tabnew" => {
                if tail.is_empty() {
                    Self::OpenEmptyBufferTab
                } else {
                    Self::OpenPathInEditor(tail.to_string())
                }
            }
            "q" | "q!" | "quit" | "quit!" | "quite" | "quite!" | "close" | "close!" => {
                Self::CloseFocusedBufferTab
            }
            "wq" | "wq!" | "x" | "x!" | "exit" => Self::WriteAndCloseFocusedBuffer,
            "qa" | "qa!" | "quitall" | "quitall!" | "qall" | "qall!" => {
                Self::CloseAllBuffersInFocusedPaneOrWorkspace
            }
            "wqa" | "wqa!" | "wqall" | "wqall!" | "xa" | "xa!" | "xall" => {
                Self::WriteAllAndCloseAllBuffers
            }
            _ => Self::PassThrough,
        }
    }
}

// ---------------------------------------------------------------------------
// Scrollbar click / drag / release plans (Wave 13-C)
// ---------------------------------------------------------------------------

/// Outcome of a scrollbar click hit-test, *after* the host has done
/// the per-pane geometry probe and reported the booleans on
/// [`ScrollbarClickContext`]. Wraps the existing
/// [`ScrollbarClickIntent`] into a richer plan that also carries the
/// jump-to-track offset (which the host applies via
/// `apply_scrollbar_display_offset`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarClickPlan {
    /// Click landed outside any band/thumb — the host returns `false`
    /// (do not consume).
    Ignore,
    /// Click landed in the editor/terminal band X-zone but the pane
    /// has no active thumb (file fits in viewport). Consume the click
    /// so it doesn't fall through to nvim's mouse handler.
    SwallowEmptyBand,
    /// Click hit the thumb — start a drag, no jump.
    StartDragOnThumb,
    /// Click hit the track (not the thumb) — start a drag *and*
    /// jump-scroll to the clicked position. The host queries
    /// `scrollbar.drag_update(mouse_y)` for the offset.
    StartDragWithJumpToTrack,
}

impl From<ScrollbarClickIntent> for ScrollbarClickPlan {
    fn from(intent: ScrollbarClickIntent) -> Self {
        match intent {
            ScrollbarClickIntent::Ignore => Self::Ignore,
            ScrollbarClickIntent::SwallowEmptyBand => Self::SwallowEmptyBand,
            ScrollbarClickIntent::StartDrag {
                jump_to_track: false,
            } => Self::StartDragOnThumb,
            ScrollbarClickIntent::StartDrag {
                jump_to_track: true,
            } => Self::StartDragWithJumpToTrack,
        }
    }
}

impl ScrollbarClickPlan {
    /// Drive the plan from the same [`ScrollbarClickContext`] used by
    /// [`ScrollbarClickIntent`] — keeps a single source of truth.
    pub fn classify(ctx: ScrollbarClickContext) -> Self {
        ctx.intent().into()
    }
}

/// Drag-update decision: given the current drag state's rich_text_id
/// (if any) and the pane's panel-state rich_text_id (if any), pick the
/// id that should receive `apply_scrollbar_display_offset`. Mirrors
/// the priority chain in
/// `Screen::handle_scrollbar_drag` so desktop + future web stay in
/// sync.
pub fn scrollbar_drag_target_rich_text_id(
    drag_state_rich_text_id: Option<usize>,
    panel_state_rich_text_id: Option<usize>,
    current_rich_text_id: usize,
) -> usize {
    active_scrollbar_drag_rich_text_id(
        drag_state_rich_text_id,
        panel_state_rich_text_id,
        current_rich_text_id,
    )
}
