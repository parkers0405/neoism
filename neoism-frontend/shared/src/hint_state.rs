//! Shared terminal hint-mode state machine.
//!
//! Wraps the existing [`crate::hint_policy`] primitives so the native and
//! web frontends can share the same `HintState` machinery (active hint
//! Rc, visible matches, label alphabet, keys pressed so far) without
//! depending on `neoism_backend` for the concrete hint config type.
//!
//! Callers parameterise [`HintState`] / [`HintMatch`] over their hint
//! config type `H`. The desktop fork passes `Rc<neoism_backend::config::hints::Hint>`;
//! the web frontend can use whatever serializable shape arrives over the
//! workspace bridge. Behaviour-bearing accessors (regex, hyperlinks,
//! post_processing, persist) come from the [`HintConfig`] trait.
//!
//! What lives here:
//! - `HintMatch<H>`: a single match span + the hint that produced it.
//! - `HintState<H>`: alphabet / labels / keys / matches state machine.
//! - `HintConfig`: trait the hint type implements for behaviour fields.
//!
//! What still lives in the host: regex compilation (`onig`), grid
//! scanning, side effects (clipboard copy, command spawn). The host
//! drives `update_matches` by handing in a `Crosswords` reference + a
//! `regex_finder` closure that yields `(start_byte, end_byte)` per line
//! for a compiled pattern.

use crate::editor::selection_model::post_process_hyperlink_uri;
use crate::hint_policy::{
    generate_hint_labels, sort_dedup_hint_matches_by_start, visible_hint_labels,
    visible_hyperlink_hint_matches,
};
use neoism_terminal_core::crosswords::grid::Dimensions;
use neoism_terminal_core::crosswords::pos::{Column, Line, Pos};
use neoism_terminal_core::crosswords::Crosswords;
use std::rc::Rc;

/// Behaviour-bearing accessors a hint config exposes to [`HintState`].
///
/// Implementors are typically simple `Rc<…>` wrappers around a config
/// struct. All methods are read-only.
pub trait HintConfig {
    /// Optional regex pattern this hint matches. `None` disables the
    /// regex scan branch (hyperlinks may still apply).
    fn regex(&self) -> Option<&str>;
    /// Whether OSC 8 hyperlinks should be surfaced as matches.
    fn hyperlinks(&self) -> bool;
    /// Whether match text should run through
    /// [`post_process_hyperlink_uri`] to trim trailing balanced
    /// punctuation.
    fn post_processing(&self) -> bool;
    /// Whether the hint stays open after a match fires (so the user can
    /// pick another match without re-entering hint mode).
    fn persist(&self) -> bool;
}

/// Concrete [`HintConfig`] for hosts that don't link the desktop
/// `neoism_backend` hint config (the web frontend). Field meanings
/// mirror `neoism_backend::config::hints::Hint`; the action side
/// effect stays host-owned.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SimpleHintConfig {
    pub regex: Option<String>,
    pub hyperlinks: bool,
    pub post_processing: bool,
    pub persist: bool,
}

impl HintConfig for SimpleHintConfig {
    fn regex(&self) -> Option<&str> {
        self.regex.as_deref()
    }
    fn hyperlinks(&self) -> bool {
        self.hyperlinks
    }
    fn post_processing(&self) -> bool {
        self.post_processing
    }
    fn persist(&self) -> bool {
        self.persist
    }
}

/// A match found by a hint.
#[derive(Debug)]
pub struct HintMatch<H> {
    /// The text that was matched.
    pub text: String,
    /// Start position of the match.
    pub start: Pos,
    /// End position of the match.
    pub end: Pos,
    /// The hint configuration that created this match.
    pub hint: Rc<H>,
}

impl<H> Clone for HintMatch<H> {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            start: self.start,
            end: self.end,
            hint: Rc::clone(&self.hint),
        }
    }
}

/// Hint-mode state machine. Owns the alphabet / labels / keys pressed
/// so far, plus the live matches and the active hint.
pub struct HintState<H> {
    active_hint: Option<Rc<H>>,
    matches: Vec<HintMatch<H>>,
    labels: Vec<Vec<char>>,
    keys: Vec<char>,
    alphabet: String,
}

impl<H> HintState<H> {
    pub fn new(alphabet: String) -> Self {
        Self {
            active_hint: None,
            matches: Vec::new(),
            labels: Vec::new(),
            keys: Vec::new(),
            alphabet,
        }
    }

    /// Whether hint mode is currently active.
    pub fn is_active(&self) -> bool {
        self.active_hint.is_some()
    }

    /// Start hint mode with the given hint configuration.
    pub fn start(&mut self, hint: Rc<H>) {
        self.active_hint = Some(hint);
        self.keys.clear();
        // matches and labels are filled in by update_matches.
    }

    /// Stop hint mode and forget all state except the alphabet.
    pub fn stop(&mut self) {
        self.active_hint = None;
        self.matches.clear();
        self.labels.clear();
        self.keys.clear();
    }

    /// Current matches list (the order matches the labels list).
    pub fn matches(&self) -> &[HintMatch<H>] {
        &self.matches
    }

    /// Keys typed so far for the current label match.
    #[allow(dead_code)]
    pub fn keys_pressed(&self) -> &[char] {
        &self.keys
    }

    /// Labels still visible given the keys typed so far.
    pub fn visible_labels(&self) -> Vec<(usize, Vec<char>)> {
        visible_hint_labels(&self.labels, &self.keys)
    }

    /// Update the alphabet used for hint labels.
    #[allow(dead_code)]
    pub fn update_alphabet(&mut self, alphabet: &str) {
        if self.alphabet != alphabet {
            self.alphabet = alphabet.to_string();
            self.keys.clear();
        }
    }
}

impl<H: HintConfig> HintState<H> {
    /// Rebuild visible matches for the current hint.
    ///
    /// `regex_finder` is invoked per line text the helper extracts from
    /// `term`; it should yield byte (start, end) spans for the
    /// configured regex. The host supplies it (this crate intentionally
    /// has no regex engine dependency).
    pub fn update_matches<F>(&mut self, term: &Crosswords, mut regex_finder: F)
    where
        F: FnMut(&str, &str) -> Vec<(usize, usize)>,
    {
        self.matches.clear();

        let hint = match &self.active_hint {
            Some(hint) => hint.clone(),
            None => return,
        };

        if let Some(regex_pattern) = hint.regex() {
            let grid = &term.grid;
            let display_offset = grid.display_offset();
            let visible_lines = grid.screen_lines();

            for line_idx in 0..visible_lines {
                let line = Line(line_idx as i32 - display_offset as i32);
                if line < Line(0) || line.0 >= grid.total_lines() as i32 {
                    continue;
                }
                let mut line_text = String::new();
                for col in 0..grid.columns() {
                    line_text.push(grid[line][Column(col)].c());
                }
                let line_text = line_text.trim_end().to_string();
                for (start, end) in regex_finder(&line_text, regex_pattern) {
                    let start_col = Column(start);
                    let mut match_text = line_text[start..end].to_string();
                    if hint.post_processing() {
                        match_text = post_process_hyperlink_uri(&match_text);
                    }
                    let end_col = Column(start + match_text.len().saturating_sub(1));
                    self.matches.push(HintMatch {
                        text: match_text,
                        start: Pos::new(line, start_col),
                        end: Pos::new(line, end_col),
                        hint: hint.clone(),
                    });
                }
            }
        }

        if hint.hyperlinks() {
            self.matches.extend(
                visible_hyperlink_hint_matches(term, hint.post_processing())
                    .into_iter()
                    .map(|span| HintMatch {
                        text: span.text,
                        start: span.start,
                        end: span.end,
                        hint: hint.clone(),
                    }),
            );
        }

        if self.matches.is_empty() {
            self.stop();
            return;
        }

        sort_dedup_hint_matches_by_start(&mut self.matches, |hint_match| {
            hint_match.start
        });

        self.labels = generate_hint_labels(&self.alphabet, self.matches.len());
    }

    /// Feed a keypress into the hint label matcher.
    ///
    /// Returns the fired match (if any). The caller is responsible for
    /// driving the side effect (open URL, copy text, etc.) using the
    /// returned `HintMatch`. Re-runs `update_matches` after a backspace.
    pub fn keyboard_input<F>(
        &mut self,
        term: &Crosswords,
        c: char,
        regex_finder: F,
    ) -> Option<HintMatch<H>>
    where
        F: FnMut(&str, &str) -> Vec<(usize, usize)>,
    {
        let persist = self
            .active_hint
            .as_ref()
            .map(|h| h.persist())
            .unwrap_or(false);
        let visible_labels = self.visible_labels();
        match crate::selection_input::hint_keystroke_decision(c, &visible_labels, persist)
        {
            crate::selection_input::HintKeystrokeDecision::PopKey => {
                self.keys.pop();
                self.update_matches(term, regex_finder);
                None
            }
            crate::selection_input::HintKeystrokeDecision::StopHintMode => {
                self.stop();
                None
            }
            crate::selection_input::HintKeystrokeDecision::FireMatch {
                match_index,
                persist,
            } => {
                let hint_match = self.matches.get(match_index)?.clone();
                if persist {
                    self.keys.clear();
                } else {
                    self.stop();
                }
                Some(hint_match)
            }
            crate::selection_input::HintKeystrokeDecision::PushKey => {
                self.keys.push(c);
                None
            }
            crate::selection_input::HintKeystrokeDecision::Ignore => None,
        }
    }
}
