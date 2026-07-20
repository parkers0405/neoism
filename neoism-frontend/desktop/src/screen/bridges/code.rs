// Native code editor bridge (the nvim-replacement surface). Part of the
// impl Screen<'_> block, mirroring the markdown bridge layout: document
// (open/save), input (keys + mouse), render (per-pane paint).

use super::super::*;

mod document;
mod input;
pub(crate) mod lsp;
mod render;
