use clap::{Parser, Subcommand};
use neochat::client::{connect_local, daemon_command, Client};
use neochat::protocol::{Action, Event, EventKind, InviteRequest, Message};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(version, about = "Neoboard: shared channels, threads, and task coordination")]
struct Cli {
    #[arg(long, global = true, env = "NEOCHAT_URL")]
    url: Option<String>,
    #[arg(long, global = true, env = "NEOCHAT_TOKEN", hide_env_values = true, requires = "url")]
    token: Option<String>,
    #[arg(long, global = true, env = "NEOCHAT_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long = "as", global = true, env = "NEOCHAT_PROFILE")]
    profile: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    /// Reuse a request ID when retrying a write to avoid duplicate results.
    #[arg(long, global = true)]
    request_id: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the local daemon in the foreground.
    Serve { #[arg(long)] bind: Option<String> },
    /// List channels visible to this credential.
    Channels,
    /// Create a channel, including before the first post.
    Channel { name: String },
    /// Read recent messages, optionally from a channel or one thread.
    Read { channel: Option<String>, #[arg(long)] thread: Option<u64> },
    Post { channel: String, text: String, #[arg(long = "label")] labels: Vec<String> },
    Reply { thread: u64, text: String },
    Edit { id: u64, text: String },
    /// Replace all labels on a message.
    Labels { id: u64, #[arg(long = "label")] labels: Vec<String> },
    React { id: u64, reaction: String, #[arg(long)] remove: bool },
    Reactions { id: Option<u64> },
    /// Upload a local file to an existing message (maximum 8 MiB).
    Attach { id: u64, file: PathBuf, #[arg(long)] name: Option<String> },
    Attachments { id: u64 },
    /// Save opaque bytes to an explicit new file; never opens or executes them.
    DownloadAttachment { id: u64, output: PathBuf },
    /// Follow activity, optionally replaying from an explicit event cursor.
    Tail { #[arg(long)] after: Option<u64>, #[arg(long)] channel: Option<String> },
    /// Wait for one matching event, then print it. Times out with exit code 1.
    Wait {
        #[arg(long)] after: Option<u64>,
        #[arg(long)] channel: Option<String>,
        #[arg(long)] thread: Option<u64>,
        #[arg(long, default_value_t = 30)] timeout_secs: u64,
    },
    #[command(subcommand)]
    Tasks(Tasks),
    Schedule {
        channel: String, text: String,
        #[arg(long)] in_secs: u64,
        #[arg(long = "label")] labels: Vec<String>,
    },
    CancelSchedule { id: u64 },
    Schedules,
    #[command(subcommand)]
    Invite(Invites),
}

#[derive(Subcommand)]
enum Tasks {
    List { #[arg(long)] channel: Option<String> },
    Create {
        channel: String, text: String,
        #[arg(long = "label")] labels: Vec<String>,
        #[arg(long = "needs")] needs: Vec<u64>,
        #[arg(long)] assignee: Option<String>,
    },
    Claim { id: u64, #[arg(long, default_value_t = 900)] lease_secs: u64 },
    Renew { id: u64, #[arg(long, default_value_t = 900)] lease_secs: u64 },
    Complete { id: u64, text: String },
    Fail { id: u64, text: String },
    Cancel { id: u64 },
}

#[derive(Subcommand)]
enum Invites {
    /// Issue a revocable bearer credential for a person or service.
    Create {
        actor: String,
        #[arg(long = "channel")] channels: Vec<String>,
        #[arg(long, default_value_t = 86_400)] valid_secs: u64,
    },
    Revoke { id: u64 },
}

fn main() -> std::process::ExitCode {
    match run(Cli::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) if err.downcast_ref::<io::Error>().is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("neochat: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    if let Commands::Serve { bind } = &cli.command {
        let mut cmd = daemon_command();
        if let Some(dir) = &cli.data_dir { cmd.arg("--data-dir").arg(dir); }
        if let Some(bind) = bind { cmd.arg("--bind").arg(bind); }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            return Err(cmd.exec().into());
        }
        #[cfg(not(unix))]
        return if cmd.status()?.success() { Ok(()) } else { Err("neochatd exited unsuccessfully".into()) };
    }

    let client = match cli.url.as_deref() {
        Some(url) => Client::new(url, cli.token.as_deref().ok_or("set NEOCHAT_TOKEN or pass --token with --url")?, cli.profile.as_deref())?,
        None if matches!(cli.command, Commands::Invite(_)) => {
            connect_local(cli.data_dir.as_deref(), None)?;
            Client::discover_admin(cli.data_dir.as_deref())?
        }
        None => connect_local(cli.data_dir.as_deref(), cli.profile.as_deref())?,
    };
    let json = cli.json;
    let action = match cli.command {
        Commands::Serve { .. } => unreachable!(),
        Commands::Channels => {
            let channels = client.snapshot()?.channels;
            return if json { print_json(&channels) } else { print_lines(channels) };
        }
        Commands::Read { channel, thread } => return show_messages(&client, json, channel.as_deref(), thread, false),
        Commands::Channel { name } => Action::CreateChannel { channel: name },
        Commands::Post { channel, text, labels } => Action::Post { channel, text, labels: parse_labels(labels)?, task: false, needs: Vec::new(), assignee: None },
        Commands::Reply { thread, text } => Action::Reply { thread, text },
        Commands::Edit { id, text } => Action::Edit { id, text },
        Commands::Attach { id, file, name } => {
            use std::io::Read;
            let name = name.or_else(|| file.file_name().and_then(|s| s.to_str()).map(str::to_owned)).ok_or("file has no UTF-8 name; pass --name")?;
            let mut bytes = Vec::new();
            std::fs::File::open(&file)?.take(neochat::protocol::MAX_ATTACHMENT_BYTES as u64 + 1).read_to_end(&mut bytes)?;
            let attachment = client.attach(id, &name, bytes)?;
            return if json { print_json(&attachment) } else { print_lines([format!("#{} {} ({} bytes)",attachment.id,attachment.name,attachment.size)]) };
        }
        Commands::Attachments { id } => {
            let attachments = client.attachments(id)?;
            return if json { print_json(&attachments) } else { print_lines(attachments.iter().map(|a| format!("#{} {} ({} bytes)",a.id,a.name,a.size))) };
        }
        Commands::DownloadAttachment { id, output } => {
            let bytes = client.download_attachment(id)?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)] {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(output)?.write_all(&bytes)?;
            return Ok(());
        }
        Commands::Labels { id, labels } => Action::SetLabels { id, labels: parse_labels(labels)? },
        Commands::React { id, reaction, remove } => Action::React { id, reaction, remove },
        Commands::Reactions { id } => {
            let reactions: Vec<_> = client.reactions()?.into_iter().filter(|r| id.is_none_or(|n| n == r.id)).collect();
            return if json { print_json(&reactions) } else { print_lines(reactions.iter().map(|r| format!("#{} #{} {} {}", r.id, r.channel, r.actor, r.reaction))) };
        }
        Commands::Tail { after, channel } => return follow(&client, json, after, channel.as_deref(), None, None),
        Commands::Wait { after, channel, thread, timeout_secs } => return follow(&client, json, after, channel.as_deref(), thread, Some(timeout_secs)),
        Commands::Schedule { channel, text, in_secs, labels } => Action::Schedule { channel, text, publish_at: future(in_secs)?, labels: parse_labels(labels)? },
        Commands::CancelSchedule { id } => Action::CancelSchedule { id },
        Commands::Schedules => {
            let schedules = client.schedules()?;
            return if json { print_json(&schedules) } else { print_lines(schedules.iter().map(|s| format!("#{} #{} at {}", s.id, s.channel, s.publish_at))) };
        }
        Commands::Invite(Invites::Create { actor, channels, valid_secs }) => {
            let invite = client.invite(&InviteRequest { actor, channels, expires_at: Some(future(valid_secs)?) })?;
            return if json { print_json(&invite) } else { print_lines([format!("invite #{} for {}", invite.id, invite.actor), invite.token]) };
        }
        Commands::Invite(Invites::Revoke { id }) => {
            client.revoke_invite(id)?;
            return if json { print_json(&serde_json::json!({"revoked":id})) } else { print_lines([format!("revoked #{id}")]) };
        }
        Commands::Tasks(tasks) => match tasks {
            Tasks::List { channel } => return show_messages(&client, json, channel.as_deref(), None, true),
            Tasks::Create { channel, text, labels, needs, assignee } => Action::Post { channel, text, labels: parse_labels(labels)?, task: true, needs, assignee },
            Tasks::Claim { id, lease_secs } => Action::Claim { id, lease_secs },
            Tasks::Renew { id, lease_secs } => Action::Renew { id, lease_secs },
            Tasks::Complete { id, text } => Action::Complete { id, text },
            Tasks::Fail { id, text } => Action::Fail { id, text },
            Tasks::Cancel { id } => Action::Cancel { id },
        },
    };
    let result = client.action_with_key(&action, cli.request_id.as_deref())?;
    if json { print_json(&result) } else if let Some(id) = result.id { print_lines([id]) } else { Ok(()) }
}

fn future(seconds: u64) -> Result<u64> {
    SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs().checked_add(seconds).ok_or_else(|| "timestamp overflow".into())
}

fn parse_labels(values: Vec<String>) -> Result<BTreeMap<String, String>> {
    values.into_iter().map(|label| {
        let (key, value) = label.split_once('=').ok_or("labels must use key=value")?;
        Ok((key.to_owned(), value.to_owned()))
    }).collect()
}

fn show_messages(client: &Client, json: bool, channel: Option<&str>, thread: Option<u64>, task: bool) -> Result<()> {
    let messages: Vec<_> = client.snapshot()?.messages.into_iter().filter(|msg| {
        (!task || msg.task.is_some()) && channel.is_none_or(|name| name == msg.channel)
            && thread.is_none_or(|id| msg.id == id || msg.parent == Some(id))
    }).collect();
    if json { print_json(&messages) } else { print_lines(messages.iter().map(format_message)) }
}

fn format_message(msg: &Message) -> String {
    let mut info = format!("#{} #{} {}", msg.id, msg.channel, msg.author);
    if let Some(parent) = msg.parent { info.push_str(&format!(" ↪ #{parent}")); }
    if let Some(task) = &msg.task { info.push_str(&format!(" [{:?}]", task.state)); }
    for (key, value) in &msg.labels { info.push_str(&format!(" {key}={value}")); }
    format!("{info}: {}", msg.text).chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect()
}

fn matches(event: &Event, channel: Option<&str>, thread: Option<u64>) -> bool {
    let name = match &event.kind {
        EventKind::Message { message } => {
            if !thread.is_none_or(|id| message.id == id || message.parent == Some(id)) { return false; }
            &message.channel
        }
        EventKind::Reaction { id, channel, .. } | EventKind::ThreadDeleted { id, channel } => {
            if !thread.is_none_or(|thread| thread == *id) { return false; }
            channel
        }
        EventKind::ChannelCreated { channel } | EventKind::Scheduled { channel, .. } | EventKind::ScheduleCanceled { channel, .. } | EventKind::SchedulePublished { channel, .. } => {
            if thread.is_some() { return false; }
            channel
        }
    };
    channel.is_none_or(|requested| requested == name)
}

fn follow(client: &Client, json: bool, after: Option<u64>, channel: Option<&str>, thread: Option<u64>, timeout: Option<u64>) -> Result<()> {
    let mut cursor = match after { Some(after) => after, None => client.snapshot()?.cursor };
    let start = std::time::Instant::now();
    loop {
        let window = match timeout {
            Some(secs) => std::time::Duration::from_secs(secs).saturating_sub(start.elapsed()).as_millis().min(15_000) as u64,
            None => 15_000,
        };
        for event in client.events(cursor, window)? {
            cursor = cursor.max(event.cursor);
            if !matches(&event, channel, thread) { continue; }
            if json {
                print_json(&event)?;
            } else {
                match &event.kind {
                    EventKind::Message { message } => print_lines([format!("{} {}", event.cursor, format_message(message))])?,
                    _ => print_json(&event)?,
                }
            }
            if timeout.is_some() { return Ok(()); }
        }
        if window == 0 { return Err("timed out waiting for an event".into()); }
    }
}

fn print_lines(lines: impl IntoIterator<Item = impl std::fmt::Display>) -> Result<()> {
    let mut stdout = io::stdout().lock();
    for line in lines { writeln!(stdout, "{line}")?; }
    stdout.flush()?;
    Ok(())
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}