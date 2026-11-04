//! The workspace's development entrypoint. Installed releases contain the compiled
//! GPUIX application directly; there is no second desktop implementation.
use std::{path::PathBuf, process::Command};
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let tui = args.first().is_some_and(|arg| arg == "tui");
    if !tui && args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("Neoism {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if !tui && args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Neoism — GPUIX workspace client\n\nUsage: bash scripts/blaze.sh run -p neoism -- [options]\n       bash scripts/blaze.sh run -p neoism -- tui [options]\n\n  tui               Start the Solid/OpenTUI terminal frontend (Bun 1.4.1+)\n  --daemon URL      Attach to a remote daemon\n  --workspace ID    Select a workspace\n  --config PATH     Read JSONC configuration\n  --maximized       Maximize the window\n\nmake build produces the standalone application; ./install.sh installs it.");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut command = if tui {
        let local_bun = root.join("neoism-frontend/tui/node_modules/.bin").join(if cfg!(windows) { "bun.exe" } else { "bun" });
        let mut command = Command::new(std::env::var_os("NEOISM_BUN").unwrap_or_else(|| if local_bun.is_file() { local_bun.into_os_string() } else { "bun".into() }));
        if let Ok(directory) = std::env::current_dir() { command.env("NEOISM_LAUNCH_DIRECTORY", directory); }
        command.current_dir(root.join("neoism-frontend/tui"))
            .arg("src/main.tsx").args(args.iter().skip(1));
        command
    } else {
        let mut command = Command::new("bash");
        command.arg(root.join("scripts/gpuix.sh")).arg("run").args(args);
        command
    };
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
