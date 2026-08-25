use serde_json::Value;

use crate::chat_session::{
    fetch_permission_requests, fetch_question_requests, fetch_session_queue,
};
use crate::chat_status::{
    first_queue_preview, question_label, queue_count, session_items, status_queue_count,
};
use crate::{response_json, BOLD, CYAN, DIM, GREEN, ORANGE, RESET, WHITE};

// ANSI Shadow figlet rendering of NEOISM.
const NEOISM_WORDMARK: &[&str] = &[
    "███╗   ██╗███████╗ ██████╗ ██╗███████╗███╗   ███╗",
    "████╗  ██║██╔════╝██╔═══██╗██║██╔════╝████╗ ████║",
    "██╔██╗ ██║█████╗  ██║   ██║██║███████╗██╔████╔██║",
    "██║╚██╗██║██╔══╝  ██║   ██║██║╚════██║██║╚██╔╝██║",
    "██║ ╚████║███████╗╚██████╔╝██║███████║██║ ╚═╝ ██║",
    "╚═╝  ╚═══╝╚══════╝ ╚═════╝ ╚═╝╚══════╝╚═╝     ╚═╝",
];

// SVG-rasterized version (from frontends/rioterm/assets/splash/neoism-wordmark.svg);
// kept here in case we want to switch back.
#[allow(dead_code)]
const NEOISM_WORDMARK_SVG: &[&str] = &[
    "                           ██                  ",
    "                                               ",
    "██████   █████   █████    ███    █████  ██████ ",
    "██   ██ ███████ ██   ██    ██   ██      █ ██ ██",
    "██   ██ ███████ ██   ██    ██     █████  █ ██ ██",
    "██   ██ ██      ██   ██    ██        ██ █ ██ ██",
    "██   ██  █████   █████   ██████ ██████  █ ██ ██",
];

pub(crate) fn print_chat_header(
    session_id: &str,
    model: Option<&str>,
    agent: Option<&str>,
    variant: Option<&str>,
    cwd: Option<&str>,
) {
    let width = crate::chat_ui::terminal_size().0 as usize;
    let session_short = session_id.chars().take(20).collect::<String>();
    let cwd_display = cwd
        .map(compact_home)
        .unwrap_or_else(|| "(workspace default)".to_string());

    println!();
    let mark_width = NEOISM_WORDMARK
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    if width >= mark_width + 4 {
        for line in NEOISM_WORDMARK.iter() {
            println!("  {BOLD}{WHITE}{line}{RESET}");
        }
    } else {
        println!("  {BOLD}{WHITE}▶ Neoism{RESET}  {DIM}headless agent runtime{RESET}");
    }
    println!();
    let rows: [(&str, String, &str); 5] = [
        ("session", session_short, DIM),
        ("cwd", cwd_display, DIM),
        ("model", model.unwrap_or("server default").to_string(), CYAN),
        (
            "agent",
            agent.unwrap_or("session default").to_string(),
            GREEN,
        ),
        (
            "think",
            variant.unwrap_or("model default").to_string(),
            WHITE,
        ),
    ];
    let label_pad = rows
        .iter()
        .map(|(label, _, _)| label.len())
        .max()
        .unwrap_or(0);
    for (label, value, color) in &rows {
        println!("   {DIM}{label:<label_pad$}{RESET}  {color}{value}{RESET}");
    }
    println!();
    println!(
        "   {DIM}/help for commands · /quit to exit · tab to switch agent · @ to mention files{RESET}"
    );
    println!();
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SessionBootstrap {
    pub(crate) should_attach: bool,
}

pub(crate) async fn print_session_bootstrap(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
) -> anyhow::Result<SessionBootstrap> {
    let statuses = response_json(
        client
            .get(format!("{server}/v2/sessions/status"))
            .send()
            .await?,
    )
    .await
    .unwrap_or(Value::Null);
    let queue = fetch_session_queue(client, server, session_id)
        .await
        .unwrap_or(Value::Null);
    let permissions = fetch_permission_requests(client, server)
        .await
        .unwrap_or(Value::Null);
    let questions = fetch_question_requests(client, server)
        .await
        .unwrap_or(Value::Null);
    let status = statuses
        .get(session_id)
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("idle");
    let queue_count =
        queue_count(&queue).unwrap_or_else(|| status_queue_count(&statuses, session_id));
    let running = queue
        .get("running")
        .and_then(Value::as_bool)
        .unwrap_or(status == "busy");
    let worker = queue
        .get("worker")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let pending_permissions = session_items(&permissions, session_id);
    let pending_questions = session_items(&questions, session_id);
    if status == "idle"
        && queue_count == 0
        && !running
        && !worker
        && pending_permissions.is_empty()
        && pending_questions.is_empty()
    {
        return Ok(SessionBootstrap::default());
    }
    let has_blocker = !pending_permissions.is_empty() || !pending_questions.is_empty();
    let should_attach =
        should_attach_existing_stream(status, running, worker, has_blocker);
    let status_color = if pending_permissions.is_empty() && pending_questions.is_empty() {
        if status == "idle" {
            GREEN
        } else {
            ORANGE
        }
    } else {
        ORANGE
    };
    println!(
        "   {DIM}status{RESET}  {status_color}{status}{RESET} {DIM}queue {queue_count}{RESET}"
    );
    if let Some(preview) = first_queue_preview(&queue) {
        println!("   {DIM}next{RESET}    {preview}");
    }
    for permission in pending_permissions {
        let title = permission
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Permission required");
        println!(
            "   {DIM}permit{RESET}  {ORANGE}{title}{RESET} {DIM}type y/a/n when prompted{RESET}"
        );
    }
    for question in pending_questions {
        let label =
            question_label(question).unwrap_or_else(|| "Question required".to_string());
        println!("   {DIM}ask{RESET}     {ORANGE}{label}{RESET}");
    }
    println!();
    Ok(SessionBootstrap { should_attach })
}

pub(crate) fn should_attach_existing_stream(
    status: &str,
    running: bool,
    worker: bool,
    has_blocker: bool,
) -> bool {
    !has_blocker && (status != "idle" || running || worker)
}

fn compact_home(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let Some(home) = std::env::var_os("HOME") else {
        return normalized;
    };
    let home = home.to_string_lossy().replace('\\', "/");
    if normalized == home {
        return "~".to_string();
    }
    normalized
        .strip_prefix(&format!("{home}/"))
        .map(|rest| format!("~/{rest}"))
        .unwrap_or(normalized)
}
