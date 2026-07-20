//! Finder panel — multi-mode search picker (files / grep / git
//! changes / ex commands).
//!
//! Verbatim port of `frontends/neoism/src/chrome/panels/finder/` into
//! the shared `neoism-ui` crate. Mechanical substitutions only:
//! the `neoism_backend::animation` spring lives in
//! [`crate::animation::CriticallyDampedSpring`], the chrome primitives
//! moved to [`crate::primitives`], and every OS-side I/O call
//! (ripgrep spawn, fff_search picker, `git status --porcelain`,
//! filesystem reads of preview content) now goes through the
//! [`crate::services::SearchService`] / [`crate::services::FilesService`]
//! capability traits so the same panel code runs on native winit and
//! web wasm.
//!
//! See `docs/CHROME_LIFT_AUDIT.md` for the wave-6 cutover plan.

pub mod file_search;
pub mod git;
pub mod grep;
pub mod modes;
pub mod policy;
pub mod render;
pub mod search;
pub mod state;
pub mod types;
pub mod update;

pub use modes::FinderMode;
pub use state::*;
use types::*;
pub use types::{ReferenceRow, SymbolRow};
