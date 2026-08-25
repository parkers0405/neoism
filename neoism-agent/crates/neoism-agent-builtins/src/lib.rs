//! First-party Agent plugins.
//!
//! These built-ins use the same public registration/runtime contracts as
//! third-party plugins. The crate deliberately does not depend on the server;
//! server-owned persistence or tool-kernel behavior is supplied through the
//! narrow host traits exposed by individual plugins.

pub mod plugin;