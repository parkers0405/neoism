//! Compact retained layout store (`LayoutStore`) driving Taffy 0.15's solver directly.
//!
//! Keeps Taffy's Flexbox, CSS Grid, Block, out-of-flow positioning, and intrinsic sizing
//! algorithms while replacing `TaffyTree`'s `SlotMap`s, 552-byte inline `Style`, 408-byte
//! inline `Cache`, dual 92-byte `Layout`s, and per-node child `Vec`s with 216-byte dense
//! `RetainedSlot`s, 64-byte `CompactStyle`, 96-byte `CompactCache`, and flat child arenas.

mod baseline_store;
mod cache;
pub mod prelude;
mod store;
mod style;
#[cfg(test)]
mod tests;
mod view;

pub use baseline_store::BaselineLayoutStore;
pub use baseline_store::baseline_storage_stats;
pub use cache::CompactCache;
pub use store::BoxLayout;
pub use store::LayoutStore;
pub use store::SlotChildIter;
pub use store::SlotId;
pub use store::StorageStats;
pub use style::CompactStyle;
pub use taffy::TaffyTree as BaselineTaffyTree;

#[cfg(not(feature = "baseline-taffy"))]
pub type LayoutEngine = LayoutStore;

#[cfg(feature = "baseline-taffy")]
pub type LayoutEngine = BaselineLayoutStore;
