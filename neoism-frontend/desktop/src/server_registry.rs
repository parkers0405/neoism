// Shared with the authenticated daemon registry bridge. All writers use the
// same interprocess lock, reload-under-lock and atomic persistence helpers.
pub use neoism_backend::server_registry::*;
