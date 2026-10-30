//! The workspace's development entrypoint. Installed releases contain the compiled
//! GPUIX application directly; there is no second desktop implementation.
use std::{path::PathBuf, process::Command};
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("Neoism {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Neoism — GPUIX workspace client\n\nUsage: bash scripts/blaze.sh run -p neoism -- [options]\n\n  --daemon URL      Attach to a remote daemon\n  --workspace ID    Select a workspace\n  --config PATH     Read JSONC configuration\n  --maximized       Maximize the window\n\nmake build produces the standalone application; ./install.sh installs it.");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut command = Command::new("bash");
    command
        .arg(root.join("scripts/gpuix.sh"))
        .arg("run")
        .args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        eprintln!("Unable to launch Neoism: {error}");
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("Unable to launch Neoism: {error}");
            std::process::exit(1);
        }
    }
}
