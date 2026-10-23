#[cfg(not(target_arch = "wasm32"))]
pub mod notes_mcp;
pub mod server_registry;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod ssh_hosts;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod system_registry;
