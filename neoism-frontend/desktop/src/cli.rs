// cli.rs was retired originally from https://github.com/alacritty/alacritty/blob/e35e5ad14fce8456afdd89f2b392b9924bb27471/alacritty/src/cli.rs
// which is licensed under Apache 2.0 license.

use clap::{Args, Parser, ValueHint};
use neoism_backend::config::Shell;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Parser, Default, Debug)]
#[clap(author, about, version)]
pub struct Cli {
    /// Options which can be passed via IPC.
    #[clap(flatten)]
    pub window_options: WindowOptions,

    /// Workspace-daemon URL to attach to (e.g.
    /// `unix:///run/user/1000/neoism.sock`, `ws://host:7878`,
    /// `wss://host:7878`). When omitted, the desktop probes the
    /// default per-user unix socket and either attaches to an
    /// existing daemon there or spawns one embedded in this process.
    /// See `embedded_daemon::EmbeddedDaemonHandle`.
    #[clap(long = "daemon-url", value_name = "URL")]
    pub daemon_url: Option<String>,

    /// Attach to a remote workspace-daemon over SSH. The value is an
    /// alias from `~/.ssh/config` (or a bare `user@host`). The desktop
    /// opens a loopback `ssh -L` forward to the remote daemon and dials
    /// it as `ws://127.0.0.1:<local-port>/session`. Takes precedence over
    /// `--daemon-url` when both are given. See `ssh_hosts`.
    #[clap(long = "ssh-host", value_name = "ALIAS")]
    pub ssh_host: Option<String>,
}

#[derive(Serialize, Deserialize, Args, Default, Clone, Debug, PartialEq, Eq)]
pub struct WindowOptions {
    /// Terminal options which can be passed via IPC.
    #[clap(flatten)]
    pub terminal_options: TerminalOptions,
}

#[derive(Serialize, Deserialize, Args, Default, Debug, Clone, PartialEq, Eq)]
pub struct TerminalOptions {
    /// Open files or neoism: invite URLs after startup.
    #[clap(long = "open", value_name = "PATH_OR_URI", value_hint = ValueHint::AnyPath)]
    pub open_paths: Vec<std::ffi::OsString>,

    /// Ask a running Neoism process to create a new OS window.
    #[clap(long)]
    pub new_window: bool,

    /// Files to open as editor tabs, or neoism: invite URLs to join.
    #[clap(value_name = "PATH_OR_URI", value_hint = ValueHint::AnyPath)]
    pub paths: Vec<std::ffi::OsString>,

    /// Command and args to execute (must be last argument).
    #[clap(short = 'e', long, allow_hyphen_values = true, num_args = 1..)]
    pub command: Vec<String>,

    /// Start the shell in the specified working directory.
    #[clap(short, long, value_hint = ValueHint::FilePath)]
    pub working_dir: Option<String>,

    /// Writes the config to a given path or the default location.
    #[clap(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
    pub write_config: Option<Option<PathBuf>>,

    /// Writes the logs to a file inside the config directory.
    #[clap(long)]
    pub enable_log_file: bool,

    /// Start window with specified title
    #[clap(long, name = "title-placeholder")]
    pub title_placeholder: Option<String>,

    /// Set the Wayland app_id or X11 WM_CLASS (Linux/BSD only)
    #[clap(long)]
    pub app_id: Option<String>,
}

/// Capture launcher authority before startup clears the environment. This is
/// intentionally not a CLI option: URI operands cannot supply focus authority.
/// Only call this for a launch containing invite URIs.
pub fn capture_invite_activation() -> Option<neoism_backend::event::InviteActivationToken> {
    #[cfg(target_os = "linux")]
    {
        use neoism_backend::event::InviteActivationToken;
        ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .find_map(InviteActivationToken::new)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Route every neoism: scheme argument to the app, including malformed invites.
/// This deliberately classifies the scheme rather than validating the join URL.
pub fn is_neoism_uri(value: &str) -> bool {
    crate::invite_links::is_invite_uri(value)
        || value
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("neoism:"))
}

/// Inspect raw argv before Clap or any command dispatch. A protocol launch has
/// only URI-delivery authority: discard every non-URI argument, even files,
/// because it may be a value injected alongside an execution/config flag.
/// Returns defaults with only opaque URI operands populated.
pub fn protocol_launch_options(arguments: &[std::ffi::OsString]) -> Option<Cli> {
    let uris: Vec<_> = arguments
        .iter()
        .filter_map(|argument| {
            let value = argument.to_str()?;
            if is_neoism_uri(value) {
                return Some(argument.clone());
            }
            // Clap also accepts --open=URI and greedy --command=URI. Do not
            // let option assignment spelling bypass the restricted entry point.
            let (option, value) = value.split_once('=')?;
            (option.starts_with('-') && is_neoism_uri(value))
                .then(|| std::ffi::OsString::from(value))
        })
        .collect();
    if uris.is_empty() {
        return None;
    }
    let mut options = Cli::default();
    options.window_options.terminal_options.paths = uris;
    Some(options)
}

/// Split before constructing PathBufs; URI spelling and percent escapes are opaque.
pub fn classify_open_arguments(
    arguments: impl IntoIterator<Item = std::ffi::OsString>,
) -> (Vec<PathBuf>, Vec<String>) {
    let mut paths = Vec::new();
    let mut uris = Vec::new();
    for argument in arguments {
        match argument.to_str() {
            Some(value) if is_neoism_uri(value) => uris.push(value.to_owned()),
            Some(value) if value.starts_with("file:") => {
                paths.push(
                    reqwest::Url::parse(value)
                        .ok()
                        .and_then(|url| url.to_file_path().ok())
                        .unwrap_or_else(|| PathBuf::from(argument)),
                );
            }
            _ => paths.push(PathBuf::from(argument)),
        }
    }
    (paths, uris)
}

#[cfg(test)]
mod uri_tests {
    use super::*;

    #[test]
    fn raw_protocol_launch_discards_all_other_authority() {
        let uri = "neoism://join?invite=https%3A%2F%2Ffast.example%2Finvite";
        for argv in [
            vec![uri, "-e", "sh", "-c", "touch /tmp/injected"],
            vec!["-e", "sh", "-c", "touch /tmp/injected", uri],
            vec!["--command", "sh", "-c", uri, "touch /tmp/injected"],
            vec![uri, "--write-config", "/tmp/injected.json"],
            vec!["--write-config", "/tmp/injected.json", uri],
            vec![
                uri,
                "--ssh-host",
                "attacker",
                "--daemon-url",
                "wss://attacker",
            ],
            vec![
                "--unknown-injected-flag",
                "looks-like-a-file.md",
                uri,
                "--new-window",
                "--working-dir",
                "/tmp",
            ],
            vec!["update", "--force", uri],
        ] {
            let raw = argv
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>();
            let options = protocol_launch_options(&raw).unwrap();
            assert!(options.daemon_url.is_none());
            assert!(options.ssh_host.is_none());
            let expected = TerminalOptions {
                paths: vec![uri.into()],
                ..TerminalOptions::default()
            };
            assert_eq!(options.window_options.terminal_options, expected);
            let (paths, uris) = classify_open_arguments(expected.paths);
            assert!(paths.is_empty());
            assert_eq!(uris, vec![uri]);
        }
        for option in ["--open", "--command", "--write-config", "--unknown"] {
            let raw = vec![format!("{option}={uri}").into(), "-e".into(), "sh".into()];
            let options = protocol_launch_options(&raw).unwrap();
            assert_eq!(
                options.window_options.terminal_options,
                TerminalOptions {
                    paths: vec![uri.into()],
                    ..TerminalOptions::default()
                }
            );
        }
        let invalid = vec!["-e".into(), "NEOISM:invalid".into(), "sh".into()];
        assert_eq!(
            protocol_launch_options(&invalid)
                .unwrap()
                .window_options
                .terminal_options
                .paths,
            vec![std::ffi::OsString::from("NEOISM:invalid")]
        );
        for argv in [
            vec!["-e", "sh", "-c", "echo normal"],
            vec!["--write-config", "/tmp/config.json"],
            vec!["notes.md"],
            vec!["--ssh-host", "trusted"],
        ] {
            let raw = argv
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>();
            assert!(protocol_launch_options(&raw).is_none());
        }
    }

    #[test]
    fn argv_uris_are_not_paths_even_when_invalid() {
        let uri =
            "neoism://join?invite=https%3A%2F%2Ffast.example%2Finvite%3Ftoken%3Dsecret";
        let args =
            Cli::try_parse_from(["neoism", "--open", uri, "notes.md", "NEOISM:broken"])
                .unwrap();
        let options = args.window_options.terminal_options;
        let (paths, uris) =
            classify_open_arguments(options.open_paths.into_iter().chain(options.paths));
        assert_eq!(paths, vec![PathBuf::from("notes.md")]);
        assert_eq!(uris, vec![uri, "NEOISM:broken"]);
        assert!(!is_neoism_uri("./neoism:file"));
        assert!(!is_neoism_uri("https://fast.example/invite"));
        #[cfg(unix)]
        {
            let (files, uris) = classify_open_arguments([
                "file:///tmp/a%20b.md".into(),
                "relative.rs".into(),
            ]);
            assert!(uris.is_empty());
            assert_eq!(
                files,
                vec![PathBuf::from("/tmp/a b.md"), PathBuf::from("relative.rs")]
            );
        }
    }
}

impl TerminalOptions {
    /// Shell override passed through the CLI.
    pub fn command(&self) -> Option<Shell> {
        let (program, args) = self.command.split_first()?;
        if program.is_empty() {
            return None;
        }

        Some(Shell {
            program: program.clone(),
            args: args.to_vec(),
        })
    }

    // pub fn override_pty_config(&self, pty_config: &mut PtyConfig) {
    //     if let Some(working_directory) = &self.working_directory {
    //         if working_directory.is_dir() {
    //             pty_config.working_directory = Some(working_directory.to_owned());
    //         } else {
    //             error!("Invalid working directory: {:?}", working_directory);
    //         }
    //     }

    //     if let Some(command) = self.command() {
    //         pty_config.shell = Some(command);
    //     }

    //     pty_config.hold |= self.hold;
    // }
}
