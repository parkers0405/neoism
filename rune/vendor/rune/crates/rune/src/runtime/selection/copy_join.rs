//! How copying joins the texts a selection takes: a tab between texts side by side (table
//! cells), else as many newlines as the two texts' [`El::copy_breaks`](crate::El::copy_breaks)
//! ask for (one by default, two for a blank line between paragraphs), and a text's
//! [`El::copy_prefix`](crate::El::copy_prefix) when the copy takes it from its start along with
//! other texts (a list item's `- `).
use std::ops::Range;

use super::{floor_boundary, trim_uncopied};
use crate::el::TextSpec;

/// Copied text, built a selected piece at a time.
#[derive(Default)]
pub(crate) struct CopyJoin {
    out: String,
    /// The newlines the last piece wants after it.
    after: u8,
    pieces: usize,
    /// The first piece's prefix, in bytes: a copy of one text alone goes without it.
    first_prefix: usize,
}

impl CopyJoin {
    /// The text so far.
    pub(crate) fn text(&self) -> &str { &self.out }

    /// Adds `piece`, what bytes `range` of `spec` copy as. `beside`: it sits beside the last
    /// piece, as a table row's next cell does. `true` when `cap` bytes cut it short.
    pub(crate) fn push(&mut self, spec: &TextSpec, range: Range<usize>, piece: &str, beside: bool, cap: usize) -> bool {
        let mut lead = String::new();
        if self.pieces > 0 {
            if beside {
                lead.push('\t');
            } else {
                let want = usize::from(self.after.max(spec.copy_breaks[0]).max(1));
                let have = self.out.bytes().rev().take_while(|&b| b == b'\n').count();
                lead.extend(std::iter::repeat_n('\n', want.saturating_sub(have)));
            }
        }
        let prefix = spec.copy_prefix.as_deref().filter(|_| from_start(spec, &range)).unwrap_or("");
        if self.pieces == 0 {
            self.first_prefix = prefix.len();
        }
        lead.push_str(prefix);
        self.pieces += 1;
        self.after = spec.copy_breaks[1];
        if self.out.len() + lead.len() > cap {
            return true;
        }
        self.out.push_str(&lead);
        if self.out.len() + piece.len() <= cap {
            self.out.push_str(piece);
            return false;
        }
        let take = floor_boundary(piece, cap.saturating_sub(self.out.len()));
        self.out.push_str(&piece[..take]);
        true
    }

    /// The copied text: a single text goes without its prefix.
    pub(crate) fn finish(mut self) -> String {
        if self.pieces == 1 && self.first_prefix > 0 {
            self.out.drain(..self.first_prefix);
        }
        self.out
    }
}

/// `range` takes `spec` from its first copied byte.
fn from_start(spec: &TextSpec, range: &Range<usize>) -> bool {
    let len = spec.content.len();
    let first = if spec.uncopied.is_empty() { 0 } else { trim_uncopied(&spec.uncopied, (0, len)).0 };
    !range.is_empty() && range.start <= first
}
