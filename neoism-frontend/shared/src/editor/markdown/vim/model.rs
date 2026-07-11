use super::*;

// ---------------------------------------------------------------------------
// Resolver types.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimOperator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimFindKind {
    /// `f` — forward onto the target char.
    To,
    /// `F` — backward onto the target char.
    ToBack,
    /// `t` — forward until just before the target char.
    Till,
    /// `T` — backward until just after the target char.
    TillBack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimMotion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    FirstNonBlank,
    LinesDownFirstNonBlank,
    LinesUpFirstNonBlank,
    WordForward {
        big: bool,
    },
    WordBack {
        big: bool,
    },
    WordEnd {
        big: bool,
    },
    WordEndBack {
        big: bool,
    },
    Find {
        kind: VimFindKind,
        target: char,
    },
    RepeatFind {
        reverse: bool,
    },
    /// One-based target line (`5G`, `5gg`, bare `gg`).
    GotoLine(usize),
    /// Bare `G`.
    LastLine,
    ParagraphForward,
    ParagraphBack,
    MatchPair,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimTextObject {
    Word { big: bool },
    Quote(char),
    Pair { open: char, close: char },
    Paragraph,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimTarget {
    Motion(VimMotion),
    Object {
        kind: VimTextObject,
        around: bool,
    },
    /// Doubled operators (`dd`, `cc`, `yy`, `>>`, `<<`).
    Lines,
    /// Visual-mode operators; the applier reads the live selection.
    Selection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimInsertKind {
    Here,
    LineStart,
    Append,
    LineEnd,
    LineBelow,
    LineAbove,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VimAction {
    Move {
        motion: VimMotion,
        count: usize,
    },
    Operate {
        op: VimOperator,
        target: VimTarget,
        count: usize,
    },
    DeleteChar {
        count: usize,
        before: bool,
    },
    ReplaceChar {
        ch: char,
        count: usize,
    },
    ToggleCase {
        count: usize,
    },
    JoinLines {
        count: usize,
    },
    Paste {
        count: usize,
        before: bool,
    },
    Undo {
        count: usize,
    },
    EnterInsert {
        kind: VimInsertKind,
    },
    EnterVisual {
        linewise: bool,
    },
    VisualSwapEnds,
    VisualToggleCase,
    VisualReplace {
        ch: char,
    },
    VisualTextObject {
        kind: VimTextObject,
        around: bool,
    },
    Search {
        reverse: bool,
        count: usize,
    },
    SearchWord {
        forward: bool,
        count: usize,
    },
    Repeat {
        count: Option<usize>,
    },
}

/// Outcome of feeding one key into the resolver.
#[derive(Clone, Debug, PartialEq)]
pub enum VimKeyFeed {
    /// Consumed; waiting for more keys.
    Pending,
    /// Consumed; the sequence was invalid and the pending state reset.
    Cancelled,
    /// Resolved into an action for the applier.
    Action(VimAction),
    /// Not a vim key — the host may fall through to its own handling.
    Unhandled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VimStage {
    #[default]
    Ready,
    /// `f`/`F`/`t`/`T` seen; waiting for the target char.
    Find(VimFindKind),
    /// `r` seen; waiting for the replacement char.
    Replace,
    /// `g` seen; waiting for `g`/`e`/`E`.
    Gee,
    /// `i`/`a` seen after an operator (or in visual); waiting for the
    /// object kind.
    Object { around: bool },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VimPending {
    pub count1: usize,
    pub operator: Option<VimOperator>,
    pub count2: usize,
    pub stage: VimStage,
}

impl VimPending {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn current_count(&self) -> usize {
        if self.operator.is_some() {
            self.count2
        } else {
            self.count1
        }
    }

    fn push_digit(&mut self, digit: usize) {
        let slot = if self.operator.is_some() {
            &mut self.count2
        } else {
            &mut self.count1
        };
        *slot = slot.saturating_mul(10).saturating_add(digit).min(1_000_000);
    }

    fn effective_count(&self) -> usize {
        self.count1.max(1).saturating_mul(self.count2.max(1))
    }

    fn count_given(&self) -> bool {
        self.count1 > 0 || self.count2 > 0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VimSearch {
    pub pattern: String,
    pub forward: bool,
    pub whole_word: bool,
}

/// Live state for the markdown pane's incremental `/` (or `?`) search.
/// Mirrors nvim incsearch: as the pattern grows the view jumps to the
/// nearest match, every occurrence lights up, and Esc restores the
/// pre-search view. Distinct from [`VimSearch`], which is the committed
/// pattern that `n`/`N` walk after the input closes.
#[derive(Clone, Debug, Default)]
pub struct MarkdownIncSearch {
    /// Pattern typed so far (empty right after the opening `/`).
    pub query: String,
    /// Opened with `?` — flips the "nearest match" preference to
    /// at/before the cursor and points a follow-up `n` backward.
    pub reverse: bool,
    /// Pre-search cursor + scroll, restored verbatim on cancel (Esc).
    pub origin_line: usize,
    pub origin_col: usize,
    pub origin_scroll_y: f32,
    pub origin_target_scroll_y: f32,
    /// Every match's start position (source line, byte col), file order.
    pub matches: Vec<(usize, usize)>,
    /// Index into `matches` of the focused match — the cursor sits here
    /// and it paints brighter. `usize::MAX` when nothing matches.
    pub current: usize,
}

/// Per-pane vim state: the pending key sequence plus the sticky pieces
/// (`;`/`,` find memory, `n`/`N` search memory, `.` repeat memory, and
/// whether the current visual selection is linewise).
#[derive(Clone, Debug, Default)]
pub struct VimState {
    pub pending: VimPending,
    pub last_find: Option<(VimFindKind, char)>,
    pub search: Option<VimSearch>,
    /// Live `/` incremental-search input (present only while the search
    /// prompt is open); `None` in every other state.
    pub incsearch: Option<MarkdownIncSearch>,
    pub last_edit: Option<VimAction>,
    pub visual_linewise: bool,
}

/// What the applier reports back to the host dispatch.
#[derive(Clone, Debug, Default)]
pub struct VimApplied {
    pub handled: bool,
    pub snap_cursor: bool,
    /// Text for the unnamed register (host clipboard); linewise content
    /// carries a trailing `'\n'`.
    pub register: Option<String>,
    /// Show the "Yanked N lines" style notification.
    pub yank_notification: bool,
}

impl VimApplied {
    pub(crate) fn motion() -> Self {
        Self {
            handled: true,
            ..Self::default()
        }
    }

    pub(crate) fn edit() -> Self {
        Self {
            handled: true,
            snap_cursor: true,
            ..Self::default()
        }
    }

    pub(crate) fn noop() -> Self {
        Self {
            handled: true,
            ..Self::default()
        }
    }
}

impl VimState {
    /// Drop any pending key sequence (Esc, mode switches, undo, …).
    /// Returns whether something was pending.
    pub fn clear_pending(&mut self) -> bool {
        let had = !self.pending.is_empty();
        self.pending = VimPending::default();
        had
    }

    /// Feed one plain-character key from Normal or Visual mode.
    pub fn feed(&mut self, ch: char, visual: bool) -> VimKeyFeed {
        match self.pending.stage {
            VimStage::Find(kind) => {
                self.last_find = Some((kind, ch));
                return self.finish_motion(VimMotion::Find { kind, target: ch }, visual);
            }
            VimStage::Replace => {
                let count = self.pending.effective_count();
                self.clear_pending();
                return VimKeyFeed::Action(if visual {
                    VimAction::VisualReplace { ch }
                } else {
                    VimAction::ReplaceChar { ch, count }
                });
            }
            VimStage::Gee => {
                self.pending.stage = VimStage::Ready;
                return match ch {
                    'g' => {
                        let line = if self.pending.count_given() {
                            self.pending.effective_count()
                        } else {
                            1
                        };
                        self.finish_motion(VimMotion::GotoLine(line), visual)
                    }
                    'e' => {
                        self.finish_motion(VimMotion::WordEndBack { big: false }, visual)
                    }
                    'E' => {
                        self.finish_motion(VimMotion::WordEndBack { big: true }, visual)
                    }
                    _ => {
                        self.clear_pending();
                        VimKeyFeed::Cancelled
                    }
                };
            }
            VimStage::Object { around } => {
                let kind = match ch {
                    'w' => VimTextObject::Word { big: false },
                    'W' => VimTextObject::Word { big: true },
                    '"' | '\'' | '`' => VimTextObject::Quote(ch),
                    '(' | ')' | 'b' => VimTextObject::Pair {
                        open: '(',
                        close: ')',
                    },
                    '[' | ']' => VimTextObject::Pair {
                        open: '[',
                        close: ']',
                    },
                    '{' | '}' | 'B' => VimTextObject::Pair {
                        open: '{',
                        close: '}',
                    },
                    '<' | '>' => VimTextObject::Pair {
                        open: '<',
                        close: '>',
                    },
                    'p' => VimTextObject::Paragraph,
                    _ => {
                        self.clear_pending();
                        return VimKeyFeed::Cancelled;
                    }
                };
                let count = self.pending.effective_count();
                let operator = self.pending.operator;
                self.clear_pending();
                return VimKeyFeed::Action(match operator {
                    Some(op) => VimAction::Operate {
                        op,
                        target: VimTarget::Object { kind, around },
                        count,
                    },
                    None => VimAction::VisualTextObject { kind, around },
                });
            }
            VimStage::Ready => {}
        }

        // Count digits. Vim rule: `0` only continues a count already in
        // progress — otherwise it is the line-start motion.
        if ch.is_ascii_digit() && (ch != '0' || self.pending.current_count() > 0) {
            self.pending.push_digit(ch as usize - '0' as usize);
            return VimKeyFeed::Pending;
        }

        if let Some(op) = operator_for_char(ch) {
            if visual {
                let count = self.pending.effective_count();
                self.clear_pending();
                return VimKeyFeed::Action(VimAction::Operate {
                    op,
                    target: VimTarget::Selection,
                    count,
                });
            }
            return match self.pending.operator {
                None => {
                    self.pending.operator = Some(op);
                    VimKeyFeed::Pending
                }
                Some(pending_op) if pending_op == op => {
                    let count = self.pending.effective_count();
                    self.clear_pending();
                    VimKeyFeed::Action(VimAction::Operate {
                        op,
                        target: VimTarget::Lines,
                        count,
                    })
                }
                Some(_) => {
                    self.clear_pending();
                    VimKeyFeed::Cancelled
                }
            };
        }

        match ch {
            'f' => {
                self.pending.stage = VimStage::Find(VimFindKind::To);
                return VimKeyFeed::Pending;
            }
            'F' => {
                self.pending.stage = VimStage::Find(VimFindKind::ToBack);
                return VimKeyFeed::Pending;
            }
            't' => {
                self.pending.stage = VimStage::Find(VimFindKind::Till);
                return VimKeyFeed::Pending;
            }
            'T' => {
                self.pending.stage = VimStage::Find(VimFindKind::TillBack);
                return VimKeyFeed::Pending;
            }
            'g' => {
                self.pending.stage = VimStage::Gee;
                return VimKeyFeed::Pending;
            }
            'G' => {
                let motion = if self.pending.count_given() {
                    VimMotion::GotoLine(self.pending.effective_count())
                } else {
                    VimMotion::LastLine
                };
                return self.finish_motion(motion, visual);
            }
            'i' | 'a' if self.pending.operator.is_some() || visual => {
                self.pending.stage = VimStage::Object { around: ch == 'a' };
                return VimKeyFeed::Pending;
            }
            _ => {}
        }

        if let Some(motion) = motion_for_char(ch) {
            return self.finish_motion(motion, visual);
        }

        if self.pending.operator.is_some() {
            self.clear_pending();
            return VimKeyFeed::Cancelled;
        }

        let count = self.pending.effective_count();
        let count_given = self.pending.count_given();
        let action = if visual {
            match ch {
                'o' => VimAction::VisualSwapEnds,
                '~' => VimAction::VisualToggleCase,
                'r' => {
                    self.pending.stage = VimStage::Replace;
                    return VimKeyFeed::Pending;
                }
                'x' | 'X' => VimAction::Operate {
                    op: VimOperator::Delete,
                    target: VimTarget::Selection,
                    count,
                },
                's' => VimAction::Operate {
                    op: VimOperator::Change,
                    target: VimTarget::Selection,
                    count,
                },
                'v' => VimAction::EnterVisual { linewise: false },
                'V' => VimAction::EnterVisual { linewise: true },
                _ => {
                    if self.clear_pending() {
                        return VimKeyFeed::Cancelled;
                    }
                    return VimKeyFeed::Unhandled;
                }
            }
        } else {
            match ch {
                'x' => VimAction::DeleteChar {
                    count,
                    before: false,
                },
                'X' => VimAction::DeleteChar {
                    count,
                    before: true,
                },
                'r' => {
                    self.pending.stage = VimStage::Replace;
                    return VimKeyFeed::Pending;
                }
                '~' => VimAction::ToggleCase { count },
                'J' => VimAction::JoinLines { count },
                's' => VimAction::Operate {
                    op: VimOperator::Change,
                    target: VimTarget::Motion(VimMotion::Right),
                    count,
                },
                'S' => VimAction::Operate {
                    op: VimOperator::Change,
                    target: VimTarget::Lines,
                    count,
                },
                'D' => VimAction::Operate {
                    op: VimOperator::Delete,
                    target: VimTarget::Motion(VimMotion::LineEnd),
                    count,
                },
                'C' => VimAction::Operate {
                    op: VimOperator::Change,
                    target: VimTarget::Motion(VimMotion::LineEnd),
                    count,
                },
                'Y' => VimAction::Operate {
                    op: VimOperator::Yank,
                    target: VimTarget::Lines,
                    count,
                },
                'p' => VimAction::Paste {
                    count,
                    before: false,
                },
                'P' => VimAction::Paste {
                    count,
                    before: true,
                },
                'u' => VimAction::Undo { count },
                'n' => VimAction::Search {
                    reverse: false,
                    count,
                },
                'N' => VimAction::Search {
                    reverse: true,
                    count,
                },
                '*' => VimAction::SearchWord {
                    forward: true,
                    count,
                },
                '#' => VimAction::SearchWord {
                    forward: false,
                    count,
                },
                '.' => VimAction::Repeat {
                    count: count_given.then_some(count),
                },
                'i' => VimAction::EnterInsert {
                    kind: VimInsertKind::Here,
                },
                'I' => VimAction::EnterInsert {
                    kind: VimInsertKind::LineStart,
                },
                'a' => VimAction::EnterInsert {
                    kind: VimInsertKind::Append,
                },
                'A' => VimAction::EnterInsert {
                    kind: VimInsertKind::LineEnd,
                },
                'o' => VimAction::EnterInsert {
                    kind: VimInsertKind::LineBelow,
                },
                'O' => VimAction::EnterInsert {
                    kind: VimInsertKind::LineAbove,
                },
                'v' => VimAction::EnterVisual { linewise: false },
                'V' => VimAction::EnterVisual { linewise: true },
                _ => {
                    if self.clear_pending() {
                        return VimKeyFeed::Cancelled;
                    }
                    return VimKeyFeed::Unhandled;
                }
            }
        };
        self.clear_pending();
        VimKeyFeed::Action(action)
    }

    fn finish_motion(&mut self, motion: VimMotion, _visual: bool) -> VimKeyFeed {
        let count = self.pending.effective_count();
        let operator = self.pending.operator;
        self.clear_pending();
        VimKeyFeed::Action(match operator {
            Some(op) => VimAction::Operate {
                op,
                target: VimTarget::Motion(motion),
                count,
            },
            None => VimAction::Move { motion, count },
        })
    }
}

// ---------------------------------------------------------------------------
// Action applier.
// ---------------------------------------------------------------------------

impl VimAction {
    /// Whether applying this action needs the host clipboard content.
    pub fn wants_paste(&self) -> bool {
        matches!(self, VimAction::Paste { .. } | VimAction::Repeat { .. })
    }

    pub(crate) fn is_repeatable(&self) -> bool {
        match self {
            VimAction::Operate { target, .. } => !matches!(target, VimTarget::Selection),
            VimAction::DeleteChar { .. }
            | VimAction::ReplaceChar { .. }
            | VimAction::ToggleCase { .. }
            | VimAction::JoinLines { .. }
            | VimAction::Paste { .. } => true,
            _ => false,
        }
    }

    pub(crate) fn with_count(&self, count: usize) -> Self {
        let mut action = self.clone();
        match &mut action {
            VimAction::Move { count: c, .. }
            | VimAction::Operate { count: c, .. }
            | VimAction::DeleteChar { count: c, .. }
            | VimAction::ReplaceChar { count: c, .. }
            | VimAction::ToggleCase { count: c }
            | VimAction::JoinLines { count: c }
            | VimAction::Paste { count: c, .. }
            | VimAction::Undo { count: c }
            | VimAction::Search { count: c, .. }
            | VimAction::SearchWord { count: c, .. } => *c = count,
            _ => {}
        }
        action
    }
}
