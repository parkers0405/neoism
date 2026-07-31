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
    Redo {
        count: usize,
    },
    EnterInsert {
        kind: VimInsertKind,
    },
    EnterVisual {
        linewise: bool,
        /// `Ctrl-V` blockwise visual. Mutually exclusive with `linewise`.
        blockwise: bool,
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
    /// `m{a-z}` — set a buffer-local mark at the cursor.
    SetMark {
        name: char,
    },
    /// `'{a-z}` (linewise) or `` `{a-z} `` (exact).
    GotoMark {
        name: char,
        linewise: bool,
    },
    /// `Ctrl-O` — older jumplist entry.
    JumpBack {
        count: usize,
    },
    /// `Ctrl-I` / Tab — newer jumplist entry.
    JumpForward {
        count: usize,
    },
    /// `q{reg}` starts recording; bare `q` while recording stops.
    MacroRecordToggle {
        /// `None` means stop. `Some(name)` starts recording into that register.
        name: Option<char>,
    },
    /// `@{reg}` / `@@`.
    MacroPlay {
        name: char,
        count: usize,
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
    /// `"` seen; waiting for the register name.
    RegisterName,
    /// `m` seen; waiting for the mark name.
    MarkSet,
    /// `'` or `` ` `` seen; waiting for the mark name.
    MarkGoto { linewise: bool },
    /// First `q` while not recording; waiting for the macro register name.
    MacroRegister,
    /// `@` seen; waiting for the macro register name (or `@` for last).
    MacroPlay,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VimPending {
    pub count1: usize,
    pub operator: Option<VimOperator>,
    pub count2: usize,
    pub stage: VimStage,
}

impl VimPending {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// nvim-style showcmd text for the status pill ("2d", "d2f", "g").
    pub fn showcmd(&self) -> String {
        let mut out = String::new();
        if self.count1 > 0 {
            out.push_str(&self.count1.to_string());
        }
        if let Some(op) = self.operator {
            out.push(match op {
                VimOperator::Delete => 'd',
                VimOperator::Change => 'c',
                VimOperator::Yank => 'y',
                VimOperator::Indent => '>',
                VimOperator::Outdent => '<',
            });
        }
        if self.count2 > 0 {
            out.push_str(&self.count2.to_string());
        }
        match self.stage {
            VimStage::Ready => {}
            VimStage::Find(kind) => out.push(match kind {
                VimFindKind::To => 'f',
                VimFindKind::ToBack => 'F',
                VimFindKind::Till => 't',
                VimFindKind::TillBack => 'T',
            }),
            VimStage::Replace => out.push('r'),
            VimStage::Gee => out.push('g'),
            VimStage::Object { around } => out.push(if around { 'a' } else { 'i' }),
            VimStage::RegisterName => out.push('"'),
            VimStage::MarkSet => out.push('m'),
            VimStage::MarkGoto { linewise } => {
                out.push(if linewise { '\'' } else { '`' })
            }
            VimStage::MacroRegister => out.push('q'),
            VimStage::MacroPlay => out.push('@'),
        }
        out
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

/// One yank/delete payload stored in a vim register.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VimRegisterValue {
    pub text: String,
    pub linewise: bool,
    pub blockwise: bool,
}

/// Named + special registers. Unnamed (`"`) is always mirrored here; the
/// host clipboard is synced from/to unnamed (and `+`/`*`) by the applier.
#[derive(Clone, Debug, Default)]
pub struct VimRegisters {
    pub unnamed: VimRegisterValue,
    /// `"0` last yank.
    pub yank: VimRegisterValue,
    /// `"1`..`"9` delete history (1 = most recent).
    pub deletes: [VimRegisterValue; 9],
    /// `"a`..`"z` (lowercase keys). Uppercase append is handled on write.
    pub named: std::collections::BTreeMap<char, VimRegisterValue>,
    /// Last played macro register, for `@@`.
    pub last_macro: Option<char>,
    /// Recorded key sequences for `q{reg}` macros (`a`..`z` only).
    pub macros: std::collections::BTreeMap<char, String>,
}

impl VimRegisters {
    pub fn get(&self, name: char) -> Option<&VimRegisterValue> {
        match name {
            '"' | '+' | '*' => Some(&self.unnamed),
            '0' => Some(&self.yank),
            '1'..='9' => {
                let idx = (name as u8 - b'1') as usize;
                Some(&self.deletes[idx])
            }
            'a'..='z' => self.named.get(&name),
            'A'..='Z' => self.named.get(&name.to_ascii_lowercase()),
            '_' => None, // black hole
            _ => None,
        }
    }

    pub fn write(&mut self, name: char, mut value: VimRegisterValue, is_yank: bool) {
        if name == '_' {
            return;
        }
        // Uppercase named registers append.
        if matches!(name, 'A'..='Z') {
            let key = name.to_ascii_lowercase();
            if let Some(existing) = self.named.get(&key) {
                let mut merged = existing.clone();
                if merged.linewise || value.linewise {
                    if !merged.text.ends_with('\n') && !merged.text.is_empty() {
                        merged.text.push('\n');
                    }
                    merged.linewise = true;
                }
                merged.blockwise = merged.blockwise || value.blockwise;
                merged.text.push_str(&value.text);
                value = merged;
            }
            self.named.insert(key, value.clone());
            self.unnamed = value;
            return;
        }
        match name {
            '"' | '+' | '*' => {
                if is_yank {
                    self.yank = value.clone();
                } else {
                    self.push_delete(value.clone());
                }
                self.unnamed = value;
            }
            '0' => {
                self.yank = value.clone();
                self.unnamed = value;
            }
            '1'..='9' => {
                self.push_delete(value.clone());
                self.unnamed = value;
            }
            'a'..='z' => {
                self.named.insert(name, value.clone());
                self.unnamed = value;
            }
            _ => {
                if is_yank {
                    self.yank = value.clone();
                } else {
                    self.push_delete(value.clone());
                }
                self.unnamed = value;
            }
        }
    }

    fn push_delete(&mut self, value: VimRegisterValue) {
        for i in (1..9).rev() {
            self.deletes[i] = self.deletes[i - 1].clone();
        }
        self.deletes[0] = value;
    }
}

/// Buffer-local mark position (line, byte col).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VimMark {
    pub line: usize,
    pub col: usize,
}

/// Per-pane vim state: the pending key sequence plus the sticky pieces
/// (`;`/`,` find memory, `n`/`N` search memory, `.` repeat memory,
/// visual selection shape, registers, marks, jumplist, macros).
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
    /// `Ctrl-V` blockwise visual (takes precedence over `visual_linewise`).
    pub visual_block: bool,
    /// `"x` prefix waiting to be consumed by the next yank/delete/paste.
    pub pending_register: Option<char>,
    pub registers: VimRegisters,
    /// Local marks `a`..`z`.
    pub marks: std::collections::BTreeMap<char, VimMark>,
    /// Jumplist entries (line, col). Index points at the current slot.
    pub jumplist: Vec<VimMark>,
    pub jumplist_idx: usize,
    /// Macro register currently recording into (`a`..`z`), if any.
    pub recording: Option<char>,
    /// Keys captured while `recording` is set (normal/visual chars only).
    pub recording_buffer: String,
    /// True while replaying a macro, so nested `q` recording is ignored.
    pub replaying_macro: bool,
}

/// What the applier reports back to the host dispatch.
#[derive(Clone, Debug, Default)]
pub struct VimApplied {
    pub handled: bool,
    pub snap_cursor: bool,
    /// Text for the unnamed register (host clipboard); linewise content
    /// carries a trailing `'\n'`.
    pub register: Option<String>,
    /// When set, the host should write `register` into the system clipboard
    /// (`+`/`*`/unnamed yank-delete path). Named-only writes leave this false.
    pub sync_clipboard: bool,
    /// Show the "Yanked N lines" style notification.
    pub yank_notification: bool,
    /// Macro key sequence the host should replay through `feed`.
    pub replay_keys: Option<String>,
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
        // Capture keys into the active macro buffer before the stage
        // machine consumes them (skip nested replay).
        if self.recording.is_some() && !self.replaying_macro {
            // Don't record the terminating `q` itself; MacroRecordToggle
            // handles stop. Prefix digits and the rest are recorded.
            let recording_stop = ch == 'q'
                && self.pending.stage == VimStage::Ready
                && self.pending.operator.is_none()
                && !self.pending.count_given();
            if !recording_stop {
                self.recording_buffer.push(ch);
            }
        }
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
            VimStage::RegisterName => {
                self.clear_pending();
                if matches!(
                    ch,
                    '"' | '+'
                        | '*'
                        | '_'
                        | '0'..='9'
                        | 'a'..='z'
                        | 'A'..='Z'
                ) {
                    self.pending_register = Some(ch);
                    return VimKeyFeed::Pending;
                }
                return VimKeyFeed::Cancelled;
            }
            VimStage::MarkSet => {
                self.clear_pending();
                if matches!(ch, 'a'..='z') {
                    return VimKeyFeed::Action(VimAction::SetMark { name: ch });
                }
                return VimKeyFeed::Cancelled;
            }
            VimStage::MarkGoto { linewise } => {
                self.clear_pending();
                if matches!(ch, 'a'..='z') {
                    return VimKeyFeed::Action(VimAction::GotoMark {
                        name: ch,
                        linewise,
                    });
                }
                return VimKeyFeed::Cancelled;
            }
            VimStage::MacroRegister => {
                self.clear_pending();
                if matches!(ch, 'a'..='z') {
                    return VimKeyFeed::Action(VimAction::MacroRecordToggle {
                        name: Some(ch),
                    });
                }
                return VimKeyFeed::Cancelled;
            }
            VimStage::MacroPlay => {
                let count = self.pending.effective_count();
                self.clear_pending();
                let name = match ch {
                    '@' => self.registers.last_macro.unwrap_or('\0'),
                    'a'..='z' => ch,
                    _ => '\0',
                };
                if name == '\0' {
                    return VimKeyFeed::Cancelled;
                }
                return VimKeyFeed::Action(VimAction::MacroPlay { name, count });
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

        // Register / mark / macro prefixes (normal + visual).
        match ch {
            '"' if self.pending.operator.is_none() => {
                self.pending.stage = VimStage::RegisterName;
                return VimKeyFeed::Pending;
            }
            'm' if self.pending.operator.is_none() && !visual => {
                self.pending.stage = VimStage::MarkSet;
                return VimKeyFeed::Pending;
            }
            '\'' if self.pending.operator.is_none() => {
                self.pending.stage = VimStage::MarkGoto { linewise: true };
                return VimKeyFeed::Pending;
            }
            '`' if self.pending.operator.is_none() => {
                self.pending.stage = VimStage::MarkGoto { linewise: false };
                return VimKeyFeed::Pending;
            }
            'q' if self.pending.operator.is_none()
                && !visual
                && !self.replaying_macro =>
            {
                if self.recording.is_some() {
                    return VimKeyFeed::Action(VimAction::MacroRecordToggle {
                        name: None,
                    });
                }
                self.pending.stage = VimStage::MacroRegister;
                return VimKeyFeed::Pending;
            }
            '@' if self.pending.operator.is_none() && !visual => {
                self.pending.stage = VimStage::MacroPlay;
                return VimKeyFeed::Pending;
            }
            _ => {}
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
                // Visual put replaces the selected range. `p` and `P` are
                // equivalent here because there is no before/after edge once
                // the selection itself is the insertion target.
                'p' | 'P' => VimAction::Paste {
                    count,
                    before: ch == 'P',
                },
                'v' => VimAction::EnterVisual {
                    linewise: false,
                    blockwise: false,
                },
                'V' => VimAction::EnterVisual {
                    linewise: true,
                    blockwise: false,
                },
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
                'v' => VimAction::EnterVisual {
                    linewise: false,
                    blockwise: false,
                },
                'V' => VimAction::EnterVisual {
                    linewise: true,
                    blockwise: false,
                },
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

    /// Control-key chords that don't produce a character feed.
    /// `key` is a lowercase letter for Ctrl-X (`'v'`, `'r'`, `'o'`, `'i'`).
    pub fn feed_ctrl(&mut self, key: char, visual: bool) -> VimKeyFeed {
        let key = key.to_ascii_lowercase();
        let count = self.pending.effective_count().max(1);
        // Cancel multi-key stages on bare control chords.
        if self.pending.stage != VimStage::Ready {
            self.clear_pending();
            return VimKeyFeed::Cancelled;
        }
        match key {
            'v' if !visual || self.pending.operator.is_none() => {
                self.clear_pending();
                VimKeyFeed::Action(VimAction::EnterVisual {
                    linewise: false,
                    blockwise: true,
                })
            }
            'r' if !visual => {
                self.clear_pending();
                VimKeyFeed::Action(VimAction::Redo { count })
            }
            'o' if !visual => {
                self.clear_pending();
                VimKeyFeed::Action(VimAction::JumpBack { count })
            }
            'i' if !visual => {
                self.clear_pending();
                VimKeyFeed::Action(VimAction::JumpForward { count })
            }
            _ => VimKeyFeed::Unhandled,
        }
    }

    /// Push the current cursor into the jumplist (dedup consecutive).
    pub fn push_jump(&mut self, line: usize, col: usize) {
        let mark = VimMark { line, col };
        if self.jumplist_idx + 1 < self.jumplist.len() {
            self.jumplist.truncate(self.jumplist_idx + 1);
        }
        if self.jumplist.last() != Some(&mark) {
            self.jumplist.push(mark);
            // Cap history.
            if self.jumplist.len() > 100 {
                let drop_n = self.jumplist.len() - 100;
                self.jumplist.drain(0..drop_n);
            }
        }
        self.jumplist_idx = self.jumplist.len().saturating_sub(1);
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
            | VimAction::Redo { count: c }
            | VimAction::JumpBack { count: c }
            | VimAction::JumpForward { count: c }
            | VimAction::MacroPlay { count: c, .. }
            | VimAction::Search { count: c, .. }
            | VimAction::SearchWord { count: c, .. } => *c = count,
            _ => {}
        }
        action
    }
}
