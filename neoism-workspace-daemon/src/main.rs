//! Thin standalone daemon entrypoint; desktop Fast children use the same runner.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    neoism_workspace_daemon::standalone::run_standalone_from(std::env::args_os())
}
