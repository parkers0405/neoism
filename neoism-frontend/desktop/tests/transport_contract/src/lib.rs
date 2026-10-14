//! Compile real desktop transport and persistent-host driver sources without
//! constructing a window or invoking any embedded/default service startup.
#[path = "../../../src/daemon_client/mod.rs"]
pub mod daemon_client;
#[allow(dead_code)]
#[path = "../../../src/embedded_daemon.rs"]
mod embedded_daemon;
#[path = "../../../src/server_registry.rs"]
pub mod server_registry;
#[allow(dead_code)]
#[path = "../../../src/service_process.rs"]
mod service_process;
#[allow(dead_code)]
#[path = "../../../src/tailscale.rs"]
mod tailscale;

// Only the persistent driver is exposed; embedded/service-owner startup is
// private, compiled for source type dependencies, and never called by tests.
pub fn launch_persistent_host(
    command: &mut std::process::Command,
    port: u16,
    secret: Option<&str>,
    commit: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<std::net::SocketAddr> {
    service_process::launch_persistent_host(command, port, secret, commit)
}
