//! Atomic, manifest-neutral acquisition of Git-backed Lua plugins.
//!
//! Checkouts are built under the extensions staging directory, validated, and
//! then moved into commit-addressed immutable directories. The JSON lockfile is
//! the sole publication point and is never changed before validation succeeds.

mod checksum;
mod error;
mod git;
mod lockfile;
mod store;

pub use checksum::{file_sha256, tree_sha256};
pub use error::{AcquisitionError, AcquisitionStage};
pub use lockfile::{LuaPluginLock, LuaPluginLockEntry, LOCKFILE_VERSION};
pub use store::{
    LuaPluginStore, PluginSpec, ProgressEvent, PruneReport, ValidatedPluginMetadata,
    ValidationContext,
};
