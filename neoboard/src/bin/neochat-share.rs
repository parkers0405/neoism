//! Host and guest entry points for the board-only sharing flow.
use clap::{Parser, Subcommand};
use neochat::{client::{connect_local, Client}, protocol::InviteRequest};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{error::Error, fs::OpenOptions, io::{Read, Write}, path::{Path, PathBuf}, process::Command, time::{SystemTime, UNIX_EPOCH}};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Parser)]
#[command(about = "Create and open private Neoboard board invites")]
struct Args {
    #[command(subcommand)]
    command: Mode,
}

#[derive(Subcommand)]
enum Mode {
    /// Create a credential file for a board behind a private HTTPS proxy.
    Invite {
        #[arg(long)] endpoint: String,
        #[arg(long)] data_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 86_400, value_parser = clap::value_parser!(u64).range(1..=2_592_000))]
        valid_secs: u64,
        #[arg(long = "channel", required_unless_present_any = ["all_channels","project"], conflicts_with_all = ["all_channels","project"])]
        channels: Vec<String>,
        #[arg(long, conflicts_with_all = ["channels","project"], required_unless_present_any = ["channels","project"])]
        all_channels: bool,
        #[arg(long)] project: Option<String>,
        actor: String,
        file: PathBuf,
    },
    /// Open the native window with a private invite file.
    Open { file: PathBuf },
    /// Check an invite and print the channels it can access.
    Check { file: PathBuf },
    /// Run a foreground Tailscale Serve session on a dedicated HTTPS port.
    Tailnet {
        #[arg(long)] data_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 8443, value_parser = clap::value_parser!(u16).range(1024..))]
        port: u16,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteFile {
    #[serde(default,skip_serializing_if="Option::is_none")]
    project: Option<String>,
    version: u8,
    endpoint: String,
    token: String,
    expires_at: u64,
}

fn now() -> Result<u64> { Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()) }

fn https_endpoint(endpoint: &str) -> Result<String> {
    let url = Url::parse(endpoint).map_err(|_| "invalid HTTPS endpoint")?;
    if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty()
        || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err("the sharing endpoint must be an HTTPS URL without credentials, query or fragment".into());
    }
    Ok(url.to_string())
}

fn read_invite(path: &Path) -> Result<InviteFile> {
    if !std::fs::symlink_metadata(path)?.is_file() { return Err("expected a regular invite file".into()); }
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() { return Err("expected an invite file".into()); }
    let mut data = Vec::new();
    file.take(8193).read_to_end(&mut data)?;
    if data.len() > 8192 { return Err("invite file is too large".into()); }
    let mut invite: InviteFile = serde_json::from_slice(&data)
        .map_err(|_| "invalid Neoboard invite file")?;
    if invite.version != 1 { return Err("unsupported Neoboard invite version".into()); }
    if invite.expires_at <= now()? { return Err("this invite has expired".into()); }
    invite.endpoint = https_endpoint(&invite.endpoint)?;
    Ok(invite)
}

fn create_invite(path: &Path, invite: &InviteFile) -> Result<()> {
    let data = serde_json::to_vec(invite)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    if let Err(err) = file.write_all(&data).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(err.into());
    }
    Ok(())
}

fn desktop() -> Command {
    let name = format!("neochat-desktop{}", std::env::consts::EXE_SUFFIX);
    let sibling = std::env::current_exe().ok().and_then(|path| path.parent().map(|dir| dir.join(&name)));
    Command::new(std::env::var_os("NEOCHAT_DESKTOP").map(PathBuf::from)
        .or_else(|| sibling.filter(|p| p.is_file())).unwrap_or_else(|| name.into()))
}

fn run(args: Args) -> Result<()> {
    match args.command {
        Mode::Invite { endpoint, data_dir, valid_secs, mut channels, project, actor, file, .. } => {
            let endpoint = https_endpoint(&endpoint)?;
            let local = connect_local(data_dir.as_deref(), None)?;
            let available = if channels.is_empty() { Vec::new() } else { local.snapshot()?.channels };
            for channel in &channels {
                if !available.contains(channel) {
                    return Err(format!("unknown channel: {channel}").into());
                }
            }
            let admin = Client::discover_admin(data_dir.as_deref())?;
            if let Some(p)=&project {
                if !neochat::PROJECTS.iter().any(|(_,id)|id==p) { return Err("unknown project".into()); }
                channels=vec![format!("{p}/*")];
            }
            let expires_at = now()? + valid_secs;
            let invite = admin.invite(&InviteRequest { actor, channels, expires_at: Some(expires_at) })?;
            let result = (|| -> Result<()> {
                // Probe with the new guest credential, never the host or admin token.
                Client::new(&endpoint, &invite.token, None)?.snapshot()?;
                create_invite(&file, &InviteFile { version: 1, endpoint, token: invite.token, expires_at,project })
            })();
            if let Err(error) = result {
                admin.revoke_invite(invite.id).map_err(|revoke| format!("{error}; couldn't revoke invite #{}: {revoke}", invite.id))?;
                return Err(error);
            }
            println!("Invite #{} written to {}", invite.id, file.display());
        }
        Mode::Check { file } => {
            let invite = read_invite(&file)?;
            let client = Client::new(&invite.endpoint, &invite.token, None)?;
            for channel in client.snapshot()?.channels { println!("#{channel}"); }
        }
        Mode::Open { file } => {
            let invite = read_invite(&file)?;
            Client::new(&invite.endpoint, &invite.token, None)?.snapshot()?;
            let mut command=desktop();
            if let Some(project)=invite.project {command.arg("--project").arg(project);}
            command.arg("--endpoint").arg(invite.endpoint).env("NEOCHAT_TOKEN", invite.token).spawn()?;
        }
        Mode::Tailnet { data_dir, port } => {
            let client = connect_local(data_dir.as_deref(), None)?;
            let target = Url::parse(client.endpoint())?;
            if target.scheme() != "http" || !matches!(target.host_str(), Some("127.0.0.1" | "[::1]")) {
                return Err("Tailscale Serve requires a loopback HTTP daemon".into());
            }
            // Refuse to change an existing serve configuration, including a Funnel.
            let status = Command::new("tailscale").args(["serve", "status", "--json"]).output()?;
            if !status.status.success() { return Err("couldn't inspect Tailscale Serve; is Tailscale running?".into()); }
            let config: serde_json::Value = if status.stdout.iter().all(u8::is_ascii_whitespace) {
                serde_json::json!({})
            } else { serde_json::from_slice(&status.stdout)? };
            if !config.is_null() && !config.as_object().is_some_and(|o| o.is_empty()) {
                return Err("Tailscale already has Serve/Funnel configuration; use an existing private proxy rather than replacing it".into());
            }
            let mut command = Command::new("tailscale");
            command.arg("serve").arg(format!("--https={port}")).arg(client.endpoint());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                return Err(command.exec().into());
            }
            #[cfg(not(unix))]
            {
                if !command.status()?.success() { return Err("Tailscale Serve exited unsuccessfully".into()); }
            }
        }
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run(Args::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => { eprintln!("neochat-share: {err}"); std::process::ExitCode::FAILURE }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_expired_and_overwritten_invites() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("guest.neochat");
        let mut invite = InviteFile { project:None,version: 1, endpoint: "http://example.com".into(), token: "guest".into(), expires_at: now().unwrap() + 60 };
        create_invite(&file, &invite).unwrap();
        assert!(read_invite(&file).is_err());
        assert!(create_invite(&file, &invite).is_err());
        let valid = dir.path().join("valid.neochat");
        invite.endpoint = "https://example.com/board".into();
        create_invite(&valid, &invite).unwrap();
        assert_eq!(read_invite(&valid).unwrap().endpoint, invite.endpoint);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&valid).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let expired = dir.path().join("expired.neochat");
        invite.expires_at = now().unwrap();
        create_invite(&expired, &invite).unwrap();
        assert!(read_invite(&expired).is_err());
    }

    #[test]
    fn sharing_requires_an_explicit_channel_scope() {
        let base = ["neochat-share", "invite", "--endpoint", "https://example.com", "guest", "invite.neochat"];
        assert!(Args::try_parse_from(base).is_err());
        assert!(Args::try_parse_from(base.into_iter().chain(["--channel", "general"])).is_ok());
        assert!(Args::try_parse_from(base.into_iter().chain(["--all-channels"])).is_ok());
        assert!(Args::try_parse_from(base.into_iter().chain(["--all-channels", "--channel", "general"])).is_err());
    }
}