//! Homegrown vim emulation for the markdown/notebook pane.
//!
//! Two halves, both pure enough to unit-test without a renderer:
//!
//! - **Key resolver** ([`VimState::feed`]): a real normal-mode state
//!   machine for `[count1] [operator] [count2] (motion | text-object |
//!   doubled-op)` sequences, replacing the old single
//!   `pending_operator: Option<char>` hack. Each key either stays
//!   pending, cancels, resolves to a typed [`VimAction`], or falls
//!   through to the host's non-vim handling.
//! - **Action applier** ([`MarkdownPane::apply_vim_action`]): translates
//!   a resolved action into the pane's existing edit/movement
//!   primitives. Every text mutation runs the same bookkeeping the
//!   pane's other edit paths run (undo snapshots, source-length
//!   recount, pending-line-edit hint, block reparse), so the CRDT
//!   binding's shadow-diff choke point picks the change up unchanged.
//!
//! Register model: the host clipboard is the unnamed register; linewise
//! content is marked by a trailing `'\n'`, matching the existing
//! `dd`/`yy`/`p` convention.

use super::helpers::*;
use super::types::*;

mod incsearch;
mod model;
mod motions;
mod pane;

pub use model::*;
pub use motions::*;

#[cfg(test)]
mod tests;
