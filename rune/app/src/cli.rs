//! The `neoism` command line. The installed app is this binary under that name, so it answers
//! everything the installers, the updater and the desktop integrations ask of `neoism` before
//! any window or daemon exists (GPUiX `cli.ts`): the version, the update helper, the runtime
//! check, and the directory or file to open.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The release this build belongs to (the root workspace version; `build.rs`).
pub const VERSION: &str = env!("NEOISM_VERSION");

const HELP: &str = "\
Usage: neoism [OPTIONS] [PATH]

Open Neoism in PATH (a folder becomes the workspace, a file opens in it).

Options:
      --working-dir <DIR>   Open DIR as the workspace
      --new-window          Open another window
      --daemon <URL>        Connect to this workspace daemon instead of the local one
  -V, --version             Print the version
  -h, --help                Print this help

Commands:
  update [--nightly]        Update Neoism (see `neoism update --help`)
  docs                      Open the bundled handbook and API reference";

/// What the window should open, from the arguments.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub working_dir: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    pub daemon: Option<String>,
    /// Arguments this layer does not own (the toolkit's headless and tooling flags).
    pub rest: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command_ {
    Version,
    Help,
    /// Hand the whole command line to the update helper.
    Update(Vec<String>),
    /// Windows: run a console program for a service without flashing a console window
    /// (`background_process::command` routes through the GUI when it is `neoism.exe`).
    Background(Vec<String>),
    Docs(Vec<String>),
    VerifyRuntime,
    Launch(Launch),
}

pub fn parse(args: &[String]) -> Command_ {
    match args.first().map(String::as_str) {
        Some("update" | "upgrade" | "self-update" | "--update" | "--macos-update-helper") => return Command_::Update(args.to_vec()),
        Some("docs") => return Command_::Docs(args[1..].to_vec()),
        Some("--neoism-internal-background-command") => return Command_::Background(args[1..].to_vec()),
        _ => {}
    }
    if args.iter().any(|a| a == "--neoism-verify-runtime") { return Command_::VerifyRuntime; }
    if args.iter().any(|a| a == "--version" || a == "-V") { return Command_::Version; }
    if args.iter().any(|a| a == "--help" || a == "-h") { return Command_::Help; }
    let mut launch = Launch::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--new-window" => {}
            "--working-dir" => launch.working_dir = iter.next().map(PathBuf::from),
            "--daemon" => launch.daemon = iter.next().cloned(),
            other if other.starts_with("--working-dir=") => launch.working_dir = Some(PathBuf::from(&other["--working-dir=".len()..])),
            other if other.starts_with("--daemon=") => launch.daemon = Some(other["--daemon=".len()..].to_owned()),
            // Everything from the first flag this layer does not own goes to the toolkit as is.
            other if other.starts_with('-') => {
                launch.rest.push(other.to_owned());
                launch.rest.extend(iter.by_ref().cloned());
            }
            path => {
                let path = PathBuf::from(path);
                if path.is_dir() && launch.working_dir.is_none() { launch.working_dir = Some(path); } else { launch.files.push(path); }
            }
        }
    }
    Command_::Launch(launch)
}

/// The directory beside the executable that holds the bundle's resources: the executable's
/// own directory, or `Contents/Resources` inside a macOS app.
pub fn resource_roots() -> Vec<PathBuf> {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()).and_then(|p| p.parent().map(Path::to_path_buf)) else { return Vec::new() };
    vec![dir.clone(), dir.join("../Resources")]
}

fn exe_name(name: &str) -> String { if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() } }

fn update_helper() -> Result<PathBuf, String> {
    resource_roots().into_iter().map(|root| root.join("web/native").join(exe_name("neoism-update-helper"))).find(|p| p.exists())
        .ok_or_else(|| "The installation is missing its update helper. Reinstall the complete Neoism bundle.".to_owned())
}

/// `neoism update ...`: the helper does the work, told which executable asked and its process
/// (`update-cli.ts`). The macOS helper re-entry passes through untouched.
fn update(args: &[String]) -> Result<i32, String> {
    let helper = update_helper()?;
    let mut command = Command::new(helper);
    command.args(args);
    if args.first().map(String::as_str) != Some("--macos-update-helper") {
        let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| e.to_string())?;
        command.arg("--invoking-exe").arg(exe).arg("--parent-pid").arg(std::process::id().to_string());
    }
    let status = command.status().map_err(|e| format!("could not start the update helper: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

/// A command whose console window, on Windows, never shows.
pub fn quiet(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn background(args: &[String]) -> Result<i32, String> {
    let (program, rest) = args.split_first().ok_or("no program to run")?;
    let status = quiet(program).args(rest).status().map_err(|e| format!("could not start {program}: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

/// Open a file or URL with the system's default application.
pub fn open_external(target: &str) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = Command::new("open"); c.arg(target); c
    } else if cfg!(windows) {
        let mut c = quiet("cmd"); c.args(["/C", "start", "", target]); c
    } else {
        let mut c = Command::new("xdg-open"); c.arg(target); c
    };
    command.spawn().map(|_| ())
}

fn docs(args: &[String]) -> Result<i32, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage: neoism docs\nOpen the bundled handbook and API reference in your default browser.\nOnline: https://neoism.ai/api/docs/");
        return Ok(0);
    }
    if !args.is_empty() { return Err("Usage: neoism docs (no options); use --help for details.".into()); }
    let page = resource_roots().into_iter().map(|root| root.join("resources/docs/api/docs/index.html")).find(|p| p.exists())
        .ok_or("This installation has no bundled documentation. Online: https://neoism.ai/api/docs/")?;
    open_external(&page.to_string_lossy()).map_err(|e| e.to_string())?;
    println!("Local documentation: {}", page.display());
    Ok(0)
}

/// The installed runtime is whole: the terminal library loads and the services this app
/// starts are beside it. Installers and the updater run this before trusting a bundle.
fn verify_runtime() -> Result<i32, String> {
    crate::ghostty::verify()?;
    if crate::startup::daemon_executable().is_none() { return Err("neoism-workspace-daemon is not beside the executable".into()); }
    println!("neoism {VERSION} runtime ok");
    Ok(0)
}

/// On Windows a GUI program has no console of its own: borrow the one it was started from, so
/// `neoism --version` typed in a terminal prints there.
fn attach_console() {
    #[cfg(windows)]
    {
        unsafe extern "system" { fn AttachConsole(process: u32) -> i32; }
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        // SAFETY: a plain Win32 call with no pointers; failure (no parent console) is fine.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS); }
    }
}

/// Run a command that needs no window. Returns the exit code, or the launch to open.
pub fn dispatch(args: &[String]) -> Result<Launch, i32> {
    let command = parse(args);
    if !matches!(command, Command_::Launch(_)) { attach_console(); }
    let finished = match command {
        Command_::Launch(launch) => return Ok(launch),
        Command_::Version => { println!("neoism {VERSION}"); Ok(0) }
        Command_::Help => { println!("{HELP}"); Ok(0) }
        Command_::Update(args) => update(&args).map_err(|e| format!("Neoism update failed: {e}")),
        Command_::Docs(args) => docs(&args).map_err(|e| format!("Neoism docs failed: {e}")),
        Command_::Background(args) => background(&args),
        Command_::VerifyRuntime => verify_runtime().map_err(|e| format!("Neoism runtime check failed: {e}")),
    };
    Err(finished.unwrap_or_else(|error| { eprintln!("{error}"); 1 }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> { list.iter().map(|s| (*s).to_owned()).collect() }

    #[test]
    fn the_updater_and_installers_get_the_commands_they_call() {
        assert_eq!(parse(&args(&["--version"])), Command_::Version);
        assert_eq!(parse(&args(&["--help"])), Command_::Help);
        assert_eq!(parse(&args(&["--neoism-verify-runtime"])), Command_::VerifyRuntime);
        // Every spelling of update goes to the helper whole, flags and all.
        for first in ["update", "upgrade", "self-update", "--update", "--macos-update-helper"] {
            let line = args(&[first, "nightly", "--help"]);
            assert_eq!(parse(&line), Command_::Update(line.clone()));
        }
        assert_eq!(parse(&args(&["docs", "--help"])), Command_::Docs(args(&["--help"])));
        // A service's hidden console program keeps its own flags, --version included.
        assert_eq!(parse(&args(&["--neoism-internal-background-command", "git", "--version"])), Command_::Background(args(&["git", "--version"])));
        assert!(VERSION.split('.').count() >= 3, "a release version, not the crate's own: {VERSION}");
    }

    #[test]
    fn a_launch_takes_its_folder_files_and_daemon_and_passes_the_rest_on() {
        let here = std::env::temp_dir();
        let dir = here.to_string_lossy().into_owned();
        // The updater relaunches with --new-window; the Windows shell passes --working-dir.
        assert_eq!(parse(&args(&["--new-window"])), Command_::Launch(Launch::default()));
        assert_eq!(parse(&args(&["--working-dir", &dir])), Command_::Launch(Launch { working_dir: Some(here.clone()), ..Launch::default() }));
        assert_eq!(parse(&args(&[&format!("--working-dir={dir}")])), Command_::Launch(Launch { working_dir: Some(here.clone()), ..Launch::default() }));
        // A folder is the workspace; anything else is a file to open in it.
        assert_eq!(parse(&args(&[&dir, "notes.md"])), Command_::Launch(Launch { working_dir: Some(here), files: vec![PathBuf::from("notes.md")], ..Launch::default() }));
        assert_eq!(parse(&args(&["--daemon=ws://host:1/session"])), Command_::Launch(Launch { daemon: Some("ws://host:1/session".into()), ..Launch::default() }));
        // Headless tooling flags are the toolkit's.
        let tooling = args(&["--scene", "welcome", "--snap", "out.png"]);
        assert_eq!(parse(&tooling), Command_::Launch(Launch { rest: tooling.clone(), ..Launch::default() }));
    }
}
